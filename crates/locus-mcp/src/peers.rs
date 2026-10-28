//! Local relay adapter. Authority comes from authenticated grant mappings loaded
//! by the operator at startup, never from initialize metadata or message args.
use super::*;
use locus_core::peers::{Peer, PeerRegistry, Registration};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::io::Read;

const MANIFEST_LIMIT: u64 = 256 * 1024;
static RELAY: OnceLock<Mutex<Relay>> = OnceLock::new();

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    engines: Vec<Engine>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Engine {
    grant_id: String,
    engine_id: String,
    task: String,
    cwd: String,
    pid: u32,
}

struct Relay {
    engines: HashMap<String, Registration>,
    registry: PeerRegistry,
    contexts: HashMap<String, TenantCtx>,
}

pub fn enabled() -> bool {
    RELAY.get().is_some()
}

pub fn validate_bind(addr: SocketAddr) -> Result<()> {
    if addr.ip() != IpAddr::V4(Ipv4Addr::LOCALHOST) {
        bail!("peer relay requires a 127.0.0.1 bind; remote overrides do not apply");
    }
    Ok(())
}

pub fn configure(path: &Path) -> Result<()> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .context("open peer engine manifest")?
        .take(MANIFEST_LIMIT + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MANIFEST_LIMIT {
        bail!("peer engine manifest exceeds size limit");
    }
    // Do not echo parser errors: a misconfigured manifest may contain secrets.
    let manifest: Manifest = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("invalid peer engine manifest"))?;
    if manifest.engines.len() > 256 {
        bail!("too many peer engines");
    }
    let mut engines = HashMap::new();
    let mut validator = PeerRegistry::default();
    for engine in manifest.engines {
        if engine.grant_id.is_empty()
            || engine.grant_id.len() > 128
            || !engine.grant_id.bytes().all(|b| b.is_ascii_hexdigit())
        {
            bail!("invalid peer grant id");
        }
        let registration = Registration {
            engine_id: engine.engine_id,
            task: engine.task,
            cwd: engine.cwd,
            pid: engine.pid,
        };
        validator
            .register(&engine.grant_id, "validation", registration.clone(), true)
            .map_err(|_| anyhow::anyhow!("invalid peer engine registration"))?;
        if engines.insert(engine.grant_id, registration).is_some() {
            bail!("duplicate peer grant mapping");
        }
    }
    RELAY
        .set(Mutex::new(Relay {
            engines,
            registry: PeerRegistry::default(),
            contexts: HashMap::new(),
        }))
        .map_err(|_| anyhow::anyhow!("peer relay already configured"))
}

fn scope_key(scope: &RequestScope) -> Option<(&str, &TenantCtx)> {
    match (&scope.anchor, &scope.tenant) {
        (AnchorScope::Http(id), Some(tenant)) => Some((id, tenant)),
        _ => None,
    }
}

impl Relay {
    fn prune(&mut self, store: &Store) {
        // One lock order everywhere: relay -> HTTP map. HTTP request handling
        // releases its map lock before entering the relay.
        let mut map = http_session_map().lock().unwrap_or_else(|e| e.into_inner());
        let mut live = HashSet::new();
        for (id, tenant) in &self.contexts {
            let scope = RequestScope {
                anchor: AnchorScope::Http(id.clone()),
                tenant: Some(tenant.clone()),
            };
            // No scope.anchor calls here: those would re-lock the HTTP map.
            if map.peek_known(id) && scope_drift(store, &scope).is_ok_and(|d| d.ok) {
                live.insert(id.clone());
            }
        }
        self.registry.prune(|id| live.contains(id));
        self.contexts.retain(|id, _| self.registry.own(id).is_ok());
    }
}

pub fn initialize(params: &Value, scope: &RequestScope) -> std::result::Result<Option<Peer>, ()> {
    let Some(relay) = RELAY.get() else {
        return Ok(None);
    };
    let (key, tenant) = scope_key(scope).ok_or(())?;
    let store = store().map_err(|_| ())?;
    if !scope_drift(&store, scope).is_ok_and(|d| d.ok) {
        return Err(());
    }
    let (session, bindings) = resolve_scope_session(scope).map_err(|_| ())?.ok_or(())?;
    let observation = anchor::observation(&session, &bindings);
    if !scope
        .anchor
        .get()
        .is_some_and(|a| a.same_identity(&observation))
    {
        return Err(());
    }
    let mut relay = relay.lock().map_err(|_| ())?;
    relay.prune(&store);
    let (registration, trusted) = if let Some(registered) = relay.engines.get(&tenant.grant_id) {
        (registered.clone(), true)
    } else if let Some(metadata) = params.pointer("/_meta/locus_peer") {
        (
            serde_json::from_value::<Registration>(metadata.clone()).map_err(|_| ())?,
            false,
        )
    } else {
        return Ok(None);
    };
    // Partition all identity dimensions, including namespaces, without exposing
    // internal HTTP IDs or grant IDs in the public peer list.
    let partition = serde_json::to_string(&json!([
        observation.binding_id,
        observation.tenant,
        observation.mode,
        observation.namespaces
    ]))
    .map_err(|_| ())?;
    let peer = relay
        .registry
        .register(key, &partition, registration, trusted)
        .map_err(|_| ())?;
    relay.contexts.insert(key.to_owned(), tenant.clone());
    Ok(Some(peer))
}

pub fn remove(key: &str) {
    if let Some(relay) = RELAY.get() {
        if let Ok(mut relay) = relay.lock() {
            relay.registry.remove(key);
            relay.contexts.remove(key);
        }
    }
}

pub fn catalog(scope: &RequestScope, bindings: &[(String, Binding)]) -> Vec<AdapterTool> {
    if !enabled() || scope_key(scope).is_none() {
        return Vec::new();
    }
    let Some(binding) = primary_binding(bindings) else {
        return Vec::new();
    };
    let page = json!({"after":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":100}});
    let entries = [
        ("list", "List registered peers in this binding and tenant.", json!({}), vec![], false),
        ("send", "Send bounded text to one peer or group thread. Locus supplies sender identity. Secret-shaped text is refused.", json!({"recipient":{"type":"string"},"thread":{"type":"string"},"body":{"type":"string","minLength":1,"maxLength":8192}}), vec!["body"], true),
        ("inbox", "Read your direct and group messages after a message cursor (non-destructive).", page.clone(), vec![], false),
        ("thread", "Create a group with members, or read an existing group using id. Only members can read or send.", json!({"members":{"type":"array","items":{"type":"string"},"minItems":1,"maxItems":16},"id":{"type":"string"},"after":page["after"],"limit":page["limit"]}), vec![], true),
    ];
    entries.into_iter().filter_map(|(suffix, description, properties, required, destructive)| {
        let mut tool = AdapterTool {
            name: format!("locus.peers.{suffix}"), description: description.into(), provider: "locus".into(), destructive,
            input_schema: json!({"type":"object","properties":properties,"required":required,"additionalProperties":false}),
        };
        let name = tool.name.clone();
        locus_core::adapters::shape_catalog_tool(binding, &name, &mut tool).then_some(tool)
    }).collect()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SendArgs {
    recipient: Option<String>,
    thread: Option<String>,
    body: String,
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PageArgs {
    #[serde(default)]
    after: u64,
    limit: Option<usize>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ThreadArgs {
    id: Option<String>,
    members: Option<Vec<String>>,
    #[serde(default)]
    after: u64,
    limit: Option<usize>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyArgs {}

fn parse<T: serde::de::DeserializeOwned>(args: &Value) -> std::result::Result<T, ()> {
    serde_json::from_value(args.clone()).map_err(|_| ())
}

pub fn call(
    scope: &RequestScope,
    session: &Session,
    bindings: &[(String, Binding)],
    name: &str,
    args: &Value,
) -> std::result::Result<Value, Value> {
    let result = execute(scope, session, bindings, name, args);
    Ok(match result {
        Ok(value) => tool_text(value, false),
        // Never echo caller text, malformed keys, peer IDs or parser errors.
        Err(()) => tool_text(
            json!({"error":"peer_operation_refused","hint":"Check registration, membership, text limits and binding policy."}),
            true,
        ),
    })
}

fn execute(
    scope: &RequestScope,
    session: &Session,
    bindings: &[(String, Binding)],
    name: &str,
    args: &Value,
) -> std::result::Result<Value, ()> {
    let relay = RELAY.get().ok_or(())?;
    let (key, tenant) = scope_key(scope).ok_or(())?;
    let binding = primary_binding(bindings).ok_or(())?;
    // Validate closed arguments and screen text BEFORE policy can create an
    // approval record. Never persist raw rejected payloads, even on deny paths.
    match name {
        "locus.peers.list" => {
            parse::<EmptyArgs>(args)?;
        }
        "locus.peers.send" => {
            let request: SendArgs = parse(args)?;
            locus_core::peers::validate_text(
                &request.body,
                locus_core::peers::MAX_BODY_BYTES,
                true,
            )
            .map_err(|_| ())?;
            if request.recipient.is_some() == request.thread.is_some() {
                return Err(());
            }
        }
        "locus.peers.inbox" => {
            parse::<PageArgs>(args)?;
        }
        "locus.peers.thread" => {
            parse::<ThreadArgs>(args)?;
        }
        _ => return Err(()),
    }
    let store = store().map_err(|_| ())?;
    let gate = ApprovalGate {
        store: &store,
        session_id: &session.session_id,
        principal: session.principal.as_deref(),
    };
    if locus_core::adapters::enforce_session_policy(binding, name, args, Some(gate))
        .map_err(|_| ())?
        .is_some()
    {
        return Err(());
    }
    let mut relay = relay.lock().map_err(|_| ())?;
    relay.prune(&store);
    if relay.contexts.get(key) != Some(tenant) || session.session_id != tenant.session_id {
        return Err(());
    }
    let registry = &mut relay.registry;
    match name {
        "locus.peers.list" => {
            parse::<EmptyArgs>(args)?;
            Ok(
                json!({"self":registry.own(key).map_err(|_| ())?, "peers":registry.list(key).map_err(|_| ())?}),
            )
        }
        "locus.peers.send" => {
            let args: SendArgs = parse(args)?;
            let message = registry
                .send(
                    key,
                    args.recipient.as_deref(),
                    args.thread.as_deref(),
                    &args.body,
                )
                .map_err(|_| ())?;
            Ok(json!({"message":message}))
        }
        "locus.peers.inbox" => {
            let args: PageArgs = parse(args)?;
            Ok(
                json!({"messages":registry.inbox(key, args.after, args.limit.unwrap_or(100)).map_err(|_| ())?}),
            )
        }
        "locus.peers.thread" => {
            let args: ThreadArgs = parse(args)?;
            match (args.id, args.members) {
                (Some(id), None) => serde_json::to_value(
                    registry
                        .thread(key, &id, args.after, args.limit.unwrap_or(100))
                        .map_err(|_| ())?,
                )
                .map_err(|_| ()),
                (None, Some(members)) if args.after == 0 && args.limit.is_none() => {
                    Ok(json!({"thread":registry.create_thread(key, members).map_err(|_| ())?}))
                }
                _ => Err(()),
            }
        }
        _ => Err(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn peer_bind_is_exact_ipv4_loopback() {
        for host in [
            "0.0.0.0:8742",
            "192.0.2.1:8742",
            "127.0.0.2:8742",
            "[::1]:8742",
            "[::ffff:127.0.0.1]:8742",
        ] {
            assert!(validate_bind(host.parse().unwrap()).is_err());
        }
        assert!(validate_bind("127.0.0.1:0".parse().unwrap()).is_ok());
    }
    #[test]
    fn closed_arguments_reject_sender_and_non_text_bodies() {
        assert!(
            parse::<SendArgs>(&json!({"recipient":"p", "body":"hello", "sender":"forged"}))
                .is_err()
        );
        assert!(parse::<SendArgs>(&json!({"recipient":"p", "body":{"text":"hello"}})).is_err());
        assert!(parse::<PageArgs>(&json!({"after":-1})).is_err());
        assert!(parse::<EmptyArgs>(&json!({"tenant":"other"})).is_err());
    }
}
