//! Broker-owned child pool. Clients keep handles, never ownership of children.
//!
//! All requests travel over the existing capability-authenticated local channel.
//! The broker loads binding material itself; a client cannot supply executable
//! configuration, a credential reference, or a worker home independently of a pin.
use super::{CompositeWorkerManager, WorkerManager};
use crate::binding::Binding;
use crate::error::{LocusError, Result};
use crate::session::{binding_fingerprint, Session};
use crate::store::Store;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum Operation {
    Ensure,
    List,
    Call { tool: String, args: Value },
}

/// Only provider-standard injection keys; never arbitrary child environment.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Delivery(pub BTreeMap<String, String>);
impl std::fmt::Debug for Delivery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Delivery([redacted])")
    }
}
impl Drop for Delivery {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        for value in self.0.values_mut() {
            value.zeroize();
        }
    }
}

impl Delivery {
    pub fn prepare(session: &Session, binding: &Binding, provider: &str) -> Result<Self> {
        let pb = binding.provider(provider).ok_or(LocusError::InvalidSeal)?;
        let resolve = pb
            .upstream
            .as_ref()
            .map(|spec| spec.expand())
            .transpose()?
            .is_some_and(|spec| spec.resolve_secrets);
        if !resolve {
            return Ok(Self::default());
        }
        let mut env =
            crate::isolation::build_isolated_env_for_provider_opts(session, binding, pb, true);
        if !env.secrets_failed.is_empty() {
            return Err(LocusError::msg(
                "shared worker credential resolution failed",
            ));
        }
        let mut delivery = Self::default();
        for key in crate::credential::inject_keys_for_binding_provider(pb)? {
            if let Some(value) = env.vars.remove(&key) {
                delivery.0.insert(key, value);
            }
        }
        Ok(delivery)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Request {
    pub session: Box<Session>,
    pub alias: String,
    pub provider: String,
    pub operation: Operation,
    pub delivery: Delivery,
    pub sandbox: bool,
    pub no_network: bool,
}

type CancellationLeases = BTreeMap<String, (Session, Arc<AtomicBool>)>;
#[derive(Clone, Default)]
pub(crate) struct Cancellations(Arc<Mutex<CancellationLeases>>);
impl Cancellations {
    fn register(&self, session: &Session) -> Arc<AtomicBool> {
        let mut leases = self.0.lock().expect("pool cancellation registry poisoned");
        let entry = leases
            .entry(session.session_id.clone())
            .or_insert_with(|| (session.clone(), Arc::new(AtomicBool::new(false))));
        if entry.0.authority_anchor != session.authority_anchor {
            entry.1.store(true, Ordering::Release);
            *entry = (session.clone(), Arc::new(AtomicBool::new(false)));
        }
        Arc::clone(&entry.1)
    }
    pub fn cancel_stale(&self, mut current: impl FnMut(&Session) -> bool) {
        self.0
            .lock()
            .expect("pool cancellation registry poisoned")
            .retain(|_, (session, flag)| {
                if current(session) {
                    true
                } else {
                    flag.store(true, Ordering::Release);
                    false
                }
            });
    }
}

pub(crate) struct Pool {
    manager: CompositeWorkerManager,
    cancellations: Cancellations,
    sessions: BTreeMap<String, Session>,
    contracts: BTreeMap<(String, String), Vec<u8>>,
}

impl Pool {
    pub fn new(cancellations: Cancellations) -> Self {
        Self {
            manager: CompositeWorkerManager::local_pool(),
            cancellations,
            sessions: BTreeMap::new(),
            contracts: BTreeMap::new(),
        }
    }

    pub fn clear(&mut self) -> Result<()> {
        let sessions: Vec<_> = self.sessions.keys().cloned().collect();
        for session in sessions {
            self.manager.teardown_session(&session)?;
        }
        self.sessions.clear();
        self.contracts.clear();
        Ok(())
    }

    pub fn reap(&mut self, mut current: impl FnMut(&Session) -> bool) -> Result<()> {
        let stale: Vec<_> = self
            .sessions
            .values()
            .filter(|session| !current(session))
            .map(|session| session.session_id.clone())
            .collect();
        for id in stale {
            self.manager.teardown_session(&id)?;
            self.sessions.remove(&id);
            self.contracts.retain(|(session, _), _| session != &id);
        }
        self.manager.reap_idle_configured()?;
        Ok(())
    }

    pub fn execute(&mut self, home: &Path, request: &Request) -> Result<Value> {
        let session = &request.session;
        let store = Store::open_existing(home)?;
        session.verify(&store.seal_key()?)?;
        session.verify_authority_shape()?;
        // Read the signed backing too: an old, once-valid unfrozen copy must not
        // override a later freeze or a deleted grant.
        let persisted = persisted_session(session)?;
        if &persisted != session.as_ref() {
            return Err(LocusError::InvalidSeal);
        }
        let binding = store.load_binding(&request.alias)?;
        validate_binding(session, &binding)?;
        let pb = binding
            .provider(&request.provider)
            .ok_or_else(|| LocusError::msg("provider absent from pinned binding"))?;
        if !pb.has_upstream() {
            return Err(LocusError::msg("shared pool requires a declared upstream"));
        }
        let config = super::composite::mcp_config_from_upstream(
            pb.upstream.as_ref().ok_or(LocusError::InvalidSeal)?,
        )?;
        if (request.sandbox && !config.sandbox)
            || (request.no_network && !config.sandbox_no_network)
        {
            return Err(LocusError::msg(
                "shared worker sandbox configuration changed; human re-pin required",
            ));
        }
        // Admission still precedes credentials and spawn. This is defense in
        // depth: MCP's call gate also performs this check before requesting a slot.
        if let Operation::Call { tool, args } = &request.operation {
            if super::provider_from_tool_name(tool) != Some(request.provider.as_str()) {
                return Err(LocusError::msg("pool tool/provider mismatch"));
            }
            if let Some(blocked) = crate::adapters::enforce_policy(&binding, tool, args, None)? {
                return Err(LocusError::msg(format!(
                    "shared worker call denied: {}",
                    blocked.content
                )));
            }
        }
        let contract_key = (session.session_id.clone(), binding.alias.clone());
        let contract = serde_json::to_vec(&binding)?;
        if self
            .contracts
            .get(&contract_key)
            .is_some_and(|old| old != &contract)
        {
            self.manager.teardown_session(&session.session_id)?;
            return Err(LocusError::msg(
                "shared worker binding changed; human re-pin required",
            ));
        }
        // Discovery never resolves credentials, spawns or extends child lifetime.
        if matches!(request.operation, Operation::List) {
            return Ok(serde_json::to_value(self.manager.list_upstream_tools(
                session,
                &binding,
                &request.provider,
            )?)?);
        }
        self.contracts.insert(contract_key, contract);
        self.sessions
            .insert(session.session_id.clone(), session.as_ref().clone());
        let allowed = crate::credential::inject_keys_for_binding_provider(pb)?;
        if request.delivery.0.keys().any(|key| !allowed.contains(key)) {
            return Err(LocusError::msg(
                "shared worker credential delivery key denied",
            ));
        }
        let slot = self.manager.ensure_delivered(
            session,
            &binding,
            &request.provider,
            request.delivery.clone(),
            self.cancellations.register(session),
        )?;
        match &request.operation {
            Operation::Ensure => Ok(serde_json::to_value(slot)?),
            Operation::List => Ok(serde_json::to_value(self.manager.list_upstream_tools(
                session,
                &binding,
                &request.provider,
            )?)?),
            Operation::Call { tool, args } => Ok(serde_json::to_value(
                self.manager.call_tool(session, &binding, tool, args)?,
            )?),
        }
    }
}

/// Read only a bounded regular backing file; never follow a replaced symlink
/// or block the authority loop opening an attacker-created FIFO.
pub(crate) fn persisted_session(session: &Session) -> Result<Session> {
    use std::io::Read;
    let backing = session.backing.as_ref().ok_or(LocusError::InvalidSeal)?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(&backing.canonical_path)?;
    let metadata = file.metadata()?;
    const LIMIT: usize = 16 * 1024;
    if !metadata.is_file() || metadata.len() > LIMIT as u64 {
        return Err(LocusError::InvalidSeal);
    }
    let mut bytes = Vec::new();
    file.take((LIMIT + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > LIMIT {
        return Err(LocusError::InvalidSeal);
    }
    Ok(serde_json::from_slice(&bytes)?)
}

fn validate_binding(session: &Session, binding: &Binding) -> Result<()> {
    let expected = if binding.alias == session.binding_alias {
        if binding.id != session.binding_id {
            return Err(LocusError::InvalidSeal);
        }
        session.binding_fp.as_ref()
    } else {
        session
            .namespaces
            .iter()
            .position(|alias| alias == &binding.alias)
            .and_then(|index| session.namespace_fps.get(index))
    };
    if expected != Some(&binding_fingerprint(binding)) {
        return Err(LocusError::msg(
            "shared worker binding differs from sealed pin",
        ));
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::binding::{BindingBody, Policy, ProviderBinding, Scope, UpstreamSpec};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    const CLIENT: &str = "workers::pool::tests::shared_pool_client_process";

    fn wait_until(mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !ready() {
            assert!(Instant::now() < deadline, "shared pool fixture timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn shared_pool_client_process() {
        let Some(root) = std::env::var_os("LOCUS_POOL_FAKE_CLIENT_ROOT") else {
            return;
        };
        let root = std::path::PathBuf::from(root);
        let id = std::env::var("LOCUS_POOL_FAKE_CLIENT_ID").unwrap();
        std::fs::write(root.join(format!("ready-{id}")), b"ready").unwrap();
        wait_until(|| root.join("release").exists());
        let store = Store::open_existing(&root).unwrap();
        let session = store.active_session().unwrap().unwrap();
        let binding = store.load_binding("pool-fixture").unwrap();
        let mut manager = CompositeWorkerManager::new();
        let slot = manager
            .ensure_provider(&session, &binding, "github")
            .unwrap();
        let tools = manager
            .list_upstream_tools(&session, &binding, "github")
            .unwrap();
        assert!(tools.iter().any(|tool| tool.name == "pool_echo"));
        let response = manager
            .call_tool(
                &session,
                &binding,
                "github.pool_echo",
                &serde_json::json!({"text":id}),
            )
            .unwrap();
        assert!(response.ok);
        assert_eq!(response.content["text"], id);
        assert_eq!(response.content["credential_present"], true);
        assert!(
            response.content["reflected"].as_str() == Some("[redacted]"),
            "credential reflection must be redacted"
        );
        std::fs::write(
            root.join(format!("pid-{id}")),
            slot.pid.unwrap().to_string(),
        )
        .unwrap();
        // Dropping one client must not terminate a sibling client's child.
    }

    fn clients(root: &Path, capability: &str, first: usize, count: usize) -> Vec<u32> {
        let fake = format!("FAKE-{}", rand::random::<u128>());
        let mut clients = Vec::new();
        for id in first..first + count {
            let child = Command::new(std::env::current_exe().unwrap())
                .args([CLIENT, "--exact", "--test-threads=1"])
                .env("LOCUS_HOME", root)
                .env("LOCUS_POOL_FAKE_CLIENT_ROOT", root)
                .env("LOCUS_POOL_FAKE_CLIENT_ID", id.to_string())
                .env("LOCUS_POOL_FAKE_CREDENTIAL", &fake)
                .env(crate::authority_anchor::EXECUTOR_CAPABILITY_ENV, capability)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            clients.push(child);
        }
        wait_until(|| (first..first + count).all(|id| root.join(format!("ready-{id}")).exists()));
        std::fs::write(root.join("release"), b"release").unwrap();
        for child in clients {
            let output = child.wait_with_output().unwrap();
            assert!(
                output.status.success(),
                "client failed: {} {}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            );
        }
        (first..first + count)
            .map(|id| {
                std::fs::read_to_string(root.join(format!("pid-{id}")))
                    .unwrap()
                    .parse()
                    .unwrap()
            })
            .collect()
    }

    fn gone(pid: u32) -> bool {
        // Signal zero probes existence only; no process environment is inspected.
        unsafe {
            libc::kill(pid as i32, 0) == -1
                && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
        }
    }

    fn fixture(idle: &str) -> (tempfile::TempDir, Store, Binding) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::env::set_var("LOCUS_HOME", root);
        std::env::set_var("LOCUS_WORKER_IDLE_SECS", idle);
        std::env::set_var("LOCUS_WORKER_SANDBOX", "0");
        std::env::set_var("LOCUS_WORKER_SANDBOX_NO_NETWORK", "0");
        let store = Store::open(root).unwrap();
        let script = r#"
import sys, json, os, time
with open(sys.argv[1], 'a') as f: f.write(str(os.getpid())+'\n')
for line in sys.stdin:
 m=json.loads(line); i=m.get('id'); method=m.get('method')
 if i is None: continue
 if method=='initialize': r={'protocolVersion':'2024-11-05','capabilities':{},'serverInfo':{'name':'FAKE-pool','version':'1'}}
 elif method=='tools/list': r={'tools':[{'name':'pool_echo','inputSchema':{'type':'object'}}]}
 elif m['params']['arguments'].get('block'):
  with open(sys.argv[1]+'.blocked','w') as f: f.write('blocked')
  sys.stdout.write('{'); sys.stdout.flush(); time.sleep(60); continue
 else: r={'text':m['params']['arguments']['text'], 'credential_present':os.environ.get('GH_TOKEN','').startswith('FAKE-'), 'reflected':os.environ.get('GH_TOKEN','')}
 print(json.dumps({'jsonrpc':'2.0','id':i,'result':r}),flush=True)
"#;
        let launches = root.join("launches");
        let mut spec = UpstreamSpec::new("python3").with_args([
            "-u",
            "-c",
            script,
            launches.to_str().unwrap(),
        ]);
        spec.resolve_secrets = true;
        let binding = Binding::from_body(BindingBody {
            id: "bnd_pool_fixture".into(),
            alias: "pool-fixture".into(),
            tenant: "FAKE-tenant".into(),
            principal: None,
            description: None,
            policy: Policy::default(),
            providers: vec![ProviderBinding {
                provider: "github".into(),
                account: "FAKE-account".into(),
                credential_ref: "env:LOCUS_POOL_FAKE_CREDENTIAL".into(),
                scope: Scope::default(),
                upstream: Some(spec),
            }],
        });
        store.save_binding(&binding).unwrap();
        (dir, store, binding)
    }

    #[test]
    fn five_process_clients_share_child_repin_and_idle_reap() {
        if !crate::test_support::run_in_isolated_process(
            module_path!(),
            "five_process_clients_share_child_repin_and_idle_reap",
        ) {
            return;
        }
        let (dir, store, binding) = fixture("1");
        let root = dir.path();
        let launches = root.join("launches");
        let pin = store.pin(&binding.alias, root, None, false).unwrap();
        let capability = store.grant_executor_capability(&pin).unwrap();
        let manager = CompositeWorkerManager::new();
        assert!(manager
            .list_upstream_tools(&pin, &binding, "github")
            .is_err());
        assert!(!launches.exists(), "discovery must never spawn");
        let pids = clients(root, &capability, 0, 5);
        assert!(pids.iter().all(|pid| *pid == pids[0]));
        assert_eq!(
            std::fs::read_to_string(&launches).unwrap().lines().count(),
            1
        );
        // Same alias still means a different generation and a fresh child.
        let new_pin = store.pin(&binding.alias, root, None, false).unwrap();
        let new_capability = store.grant_executor_capability(&new_pin).unwrap();
        let fresh = clients(root, &new_capability, 5, 1)[0];
        assert_ne!(fresh, pids[0]);
        wait_until(|| gone(pids[0]));
        assert_eq!(
            std::fs::read_to_string(&launches).unwrap().lines().count(),
            2
        );
        // No new requests drive this reap: the broker's background sweep does.
        wait_until(|| gone(fresh));
        let after_idle = clients(root, &new_capability, 6, 1)[0];
        assert_ne!(after_idle, fresh);
        assert_eq!(
            std::fs::read_to_string(&launches).unwrap().lines().count(),
            3
        );
        // Previously supervised clients cannot regain access using an old pin.
        std::env::set_var(
            "LOCUS_POOL_FAKE_CREDENTIAL",
            format!("FAKE-{}", rand::random::<u128>()),
        );
        std::env::set_var(
            crate::authority_anchor::EXECUTOR_CAPABILITY_ENV,
            &capability,
        );
        let endpoint_request = crate::authority_anchor::worker_request::<super::super::WorkerSlot>(
            &pin,
            &binding,
            "github",
            Operation::Ensure,
        );
        let error = endpoint_request.unwrap_err();
        assert!(
            error.to_string().contains("stale"),
            "stale client must fail at the authority gate"
        );
        assert_eq!(
            std::fs::read_to_string(&launches).unwrap().lines().count(),
            3
        );
        crate::authority_anchor::retire_for_test(root).unwrap();
        wait_until(|| gone(after_idle));
    }
    #[test]
    fn pin_change_interrupts_partial_inflight_response() {
        if !crate::test_support::run_in_isolated_process(
            module_path!(),
            "pin_change_interrupts_partial_inflight_response",
        ) {
            return;
        }
        let (dir, store, binding) = fixture("60");
        let root = dir.path();
        std::env::set_var(
            "LOCUS_POOL_FAKE_CREDENTIAL",
            format!("FAKE-{}", rand::random::<u128>()),
        );
        let pin = store.pin(&binding.alias, root, None, false).unwrap();
        std::env::set_var(
            crate::authority_anchor::EXECUTOR_CAPABILITY_ENV,
            store.grant_executor_capability(&pin).unwrap(),
        );
        let mut manager = CompositeWorkerManager::new();
        let slot = manager.ensure_provider(&pin, &binding, "github").unwrap();
        std::thread::scope(|scope| {
            let call = scope.spawn(|| {
                manager.call_tool(
                    &pin,
                    &binding,
                    "github.pool_echo",
                    &serde_json::json!({"block":true}),
                )
            });
            wait_until(|| root.join("launches.blocked").exists());
            // Revocation must proceed while the upstream is in a blocking read.
            store.pin(&binding.alias, root, None, false).unwrap();
            assert!(call.join().unwrap().is_err());
        });
        wait_until(|| gone(slot.pid.unwrap()));
        assert_eq!(
            std::fs::read_to_string(root.join("launches"))
                .unwrap()
                .lines()
                .count(),
            1
        );
        crate::authority_anchor::retire_for_test(root).unwrap();
    }

    #[test]
    fn idle_discovery_does_not_extend_lifetime_or_spawn() {
        if !crate::test_support::run_in_isolated_process(
            module_path!(),
            "idle_discovery_does_not_extend_lifetime_or_spawn",
        ) {
            return;
        }
        let (dir, store, binding) = fixture("1");
        let root = dir.path();
        std::env::set_var(
            "LOCUS_POOL_FAKE_CREDENTIAL",
            format!("FAKE-{}", rand::random::<u128>()),
        );
        let pin = store.pin(&binding.alias, root, None, false).unwrap();
        std::env::set_var(
            crate::authority_anchor::EXECUTOR_CAPABILITY_ENV,
            store.grant_executor_capability(&pin).unwrap(),
        );
        let mut manager = CompositeWorkerManager::new();
        let slot = manager.ensure_provider(&pin, &binding, "github").unwrap();
        wait_until(|| {
            let _ = manager.list_upstream_tools(&pin, &binding, "github");
            gone(slot.pid.unwrap())
        });
        assert!(manager
            .list_upstream_tools(&pin, &binding, "github")
            .is_err());
        assert_eq!(
            std::fs::read_to_string(root.join("launches"))
                .unwrap()
                .lines()
                .count(),
            1
        );
        crate::authority_anchor::retire_for_test(root).unwrap();
    }
    #[test]
    fn delivery_and_binding_tampering_fail_before_spawn() {
        if !crate::test_support::run_in_isolated_process(
            module_path!(),
            "delivery_and_binding_tampering_fail_before_spawn",
        ) {
            return;
        }
        let (dir, store, binding) = fixture("60");
        let root = dir.path();
        let pin = store.pin(&binding.alias, root, None, false).unwrap();
        let mut pool = Pool::new(Cancellations::default());
        let fake = format!("FAKE-{}", rand::random::<u128>());
        let mut request = Request {
            session: Box::new(pin.clone()),
            alias: binding.alias.clone(),
            provider: "github".into(),
            operation: Operation::Ensure,
            delivery: Delivery(BTreeMap::from([("AWS_PROFILE".into(), fake.clone())])),
            sandbox: false,
            no_network: false,
        };
        assert!(!format!("{:?}", request.delivery).contains(&fake));
        assert!(pool.execute(root, &request).is_err());
        assert!(!root.join("launches").exists());
        request.delivery = Delivery::default();
        request.session.worker_home = root.join("wrong-worker").display().to_string();
        assert!(pool.execute(root, &request).is_err());
        assert!(!root.join("wrong-worker").exists());
        *request.session = pin;
        request.alias = "outside-pin".into();
        let mut other = binding.clone();
        other.alias = request.alias.clone();
        store.save_binding(&other).unwrap();
        assert!(pool.execute(root, &request).is_err());
        assert!(!root.join("launches").exists());
        crate::authority_anchor::retire_for_test(root).unwrap();
    }

    #[test]
    fn changed_delivery_does_not_reuse_or_replay_live_child() {
        if !crate::test_support::run_in_isolated_process(
            module_path!(),
            "changed_delivery_does_not_reuse_or_replay_live_child",
        ) {
            return;
        }
        let (dir, store, binding) = fixture("60");
        let root = dir.path();
        std::env::set_var(
            "LOCUS_POOL_FAKE_CREDENTIAL",
            format!("FAKE-{}", rand::random::<u128>()),
        );
        let pin = store.pin(&binding.alias, root, None, false).unwrap();
        std::env::set_var(
            crate::authority_anchor::EXECUTOR_CAPABILITY_ENV,
            store.grant_executor_capability(&pin).unwrap(),
        );
        let mut manager = CompositeWorkerManager::new();
        let slot = manager.ensure_provider(&pin, &binding, "github").unwrap();
        std::env::set_var(
            "LOCUS_POOL_FAKE_CREDENTIAL",
            format!("FAKE-{}", rand::random::<u128>()),
        );
        assert!(manager.ensure_provider(&pin, &binding, "github").is_err());
        assert!(manager
            .call_tool(
                &pin,
                &binding,
                "github.pool_echo",
                &serde_json::json!({"text":"changed"})
            )
            .is_err());
        assert!(!gone(slot.pid.unwrap()));
        assert_eq!(
            std::fs::read_to_string(root.join("launches"))
                .unwrap()
                .lines()
                .count(),
            1
        );
        crate::authority_anchor::retire_for_test(root).unwrap();
        wait_until(|| gone(slot.pid.unwrap()));
    }
    #[test]
    fn renewal_changes_generation_even_when_session_id_is_unchanged() {
        if !crate::test_support::run_in_isolated_process(
            module_path!(),
            "renewal_changes_generation_even_when_session_id_is_unchanged",
        ) {
            return;
        }
        let (dir, store, mut binding) = fixture("60");
        let root = dir.path();
        binding.policy.max_session_lifetime = Some("12h".into());
        store.save_binding(&binding).unwrap();
        std::env::set_var(
            "LOCUS_POOL_FAKE_CREDENTIAL",
            format!("FAKE-{}", rand::random::<u128>()),
        );
        let pin = store
            .pin_with_ttl(
                &binding.alias,
                root,
                None,
                false,
                Some(chrono::Duration::minutes(1)),
            )
            .unwrap();
        std::env::set_var(
            crate::authority_anchor::EXECUTOR_CAPABILITY_ENV,
            store.grant_executor_capability(&pin).unwrap(),
        );
        let mut manager = CompositeWorkerManager::new();
        let old = manager.ensure_provider(&pin, &binding, "github").unwrap();
        let renewed = store.renew_active().unwrap().session;
        assert_eq!(renewed.session_id, pin.session_id);
        assert_ne!(renewed.authority_anchor, pin.authority_anchor);
        std::env::set_var(
            crate::authority_anchor::EXECUTOR_CAPABILITY_ENV,
            store.grant_executor_capability(&renewed).unwrap(),
        );
        let new = manager
            .ensure_provider(&renewed, &binding, "github")
            .unwrap();
        assert_ne!(old.pid, new.pid);
        wait_until(|| gone(old.pid.unwrap()));
        assert_eq!(
            std::fs::read_to_string(root.join("launches"))
                .unwrap()
                .lines()
                .count(),
            2
        );
        crate::authority_anchor::retire_for_test(root).unwrap();
        wait_until(|| gone(new.pid.unwrap()));
    }
}
