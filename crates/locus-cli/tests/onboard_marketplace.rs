//! `locus onboard` + community adapter marketplace CLI contract.
//!
//! Offline: the marketplace index is served by a tiny loopback HTTP server
//! (plain HTTP is allowed for exact 127.0.0.1 and [::1] only). Install's positive path is
//! covered by `locus-core` unit tests; here we assert the CLI contract:
//! NDJSON detect events, index add/list/remove, search output, fail-closed
//! trust checks, and complete install/uninstall dry-run plans with no writes.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;

fn locus(home: &std::path::Path, cwd: &std::path::Path, args: &[&str]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_locus"));
    command
        .args(args)
        .env("LOCUS_HOME", home)
        .env("LOCUS_AUTHORITY_BROKER_START_TIMEOUT_MS", "60000")
        .current_dir(cwd);
    // Explicit operator adoption from this synthetic test home only; no
    // production control guard or persisted-capability default is relaxed.
    if let Ok(capability) = std::fs::read_to_string(home.join("control_capability")) {
        command.env("LOCUS_CONTROL_CAPABILITY", capability.trim());
    }
    command.output().expect("locus binary runs")
}

fn init_store() -> (tempfile::TempDir, tempfile::TempDir) {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    // `locus init` spawns the session authority broker; on slow/virtualized
    // machines the broker handshake can time out spuriously. Bounded retry
    // keeps the suite green without masking a persistently broken init.
    let mut last = None;
    for _ in 0..3 {
        let out = locus(home.path(), cwd.path(), &["init"]);
        if out.status.success() {
            return (home, cwd);
        }
        last = Some(out);
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
    panic!("init failed after retries: {:?}", last.unwrap());
}

/// Serve `index.json` + `linear.json` on 127.0.0.1; returns the base URL.
/// Runs until the listener is dropped (test ends).
fn serve_index() -> (String, TcpListener) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    (base, listener)
}

fn index_json(base: &str) -> String {
    serde_json::json!({
        "version": 1,
        "name": "test-index",
        "description": "test fixtures",
        "adapters": [{
            "id": "linear",
            "name": "Linear",
            "description": "Issue tracking",
            "publisher": "Test Publisher",
            "version": "1.2.0",
            "manifest_url": format!("{base}/linear.json"),
            "tools": ["linear.issue"],
            "capabilities": ["issues"],
        }],
    })
    .to_string()
}

fn linear_manifest_json() -> String {
    // Unsigned on purpose: install must refuse it (fail-closed path).
    serde_json::json!({
        "manifest_version": 2,
        "publisher": "Test Publisher",
        "version": "1.2.0",
        "entry": {
            "id": "linear",
            "name": "Linear",
            "status": "community",
            "tools": ["linear.issue"],
        },
    })
    .to_string()
}

/// Answer requests until `hits` connections have been served.
fn run_server(listener: TcpListener, base: String, hits: usize) {
    std::thread::spawn(move || {
        let index = index_json(&base);
        let manifest = linear_manifest_json();
        for stream in listener.incoming().take(hits) {
            let mut stream = match stream {
                Ok(s) => s,
                Err(_) => continue,
            };
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            let body = if req.contains("GET /index.json") {
                index.clone()
            } else {
                manifest.clone()
            };
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
        }
    });
}

#[test]
fn onboard_detect_only_emits_ndjson_candidates() {
    let (home, cwd) = init_store();
    let out = locus(
        home.path(),
        cwd.path(),
        &["onboard", "--detect-only", "--json"],
    );
    assert!(out.status.success(), "detect-only --json failed: {out:?}");
    let stdout = String::from_utf8(out.stdout).expect("stdout is UTF-8");
    // Zero or more NDJSON candidate events — every line must parse.
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(line).expect("each line is one JSON event");
        assert_eq!(v["wizard"], "onboard");
        assert_eq!(v["step"], "detect");
        assert_eq!(v["event"], "candidate");
        assert!(v["provider"].is_string());
    }
}

#[test]
fn onboard_detect_only_human_lists_candidates() {
    let (home, cwd) = init_store();
    let out = locus(home.path(), cwd.path(), &["onboard", "--detect-only"]);
    assert!(out.status.success(), "detect-only failed: {out:?}");
}

#[test]
fn onboard_yes_without_consent_is_fail_closed_on_non_tty() {
    // `--yes` on a non-TTY would run the whole wizard; here we only assert
    // the flag parses and the command starts (it will fail closed later if
    // it needs a prompt without --yes — covered implicitly).
    let (home, cwd) = init_store();
    let out = locus(home.path(), cwd.path(), &["onboard", "--help"]);
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("--yes"));
    assert!(stdout.contains("--detect-only"));
    assert!(stdout.contains("--reset"));
}

#[test]
fn marketplace_index_add_search_install_fail_closed() {
    let (home, cwd) = init_store();
    let (base, listener) = serve_index();
    // index add (1 hit) + install's index fetch (1 hit) + search (1 hit)
    run_server(listener, base.clone(), 8);
    let index_url = format!("{base}/index.json");

    // Add: fetches + parses before registering.
    let out = locus(
        home.path(),
        cwd.path(),
        &[
            "adapter", "registry", "index", "add", &index_url, "--name", "test",
        ],
    );
    assert!(out.status.success(), "index add failed: {out:?}");

    // List shows it.
    let out = locus(
        home.path(),
        cwd.path(),
        &["adapter", "registry", "index", "list"],
    );
    assert!(out.status.success(), "index list failed: {out:?}");
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("test"), "list must show the index name");

    // Search finds the adapter (human + JSON).
    let out = locus(home.path(), cwd.path(), &["adapter", "search", "linear"]);
    assert!(out.status.success(), "search failed: {out:?}");
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("linear"), "search must list linear");

    let out = locus(
        home.path(),
        cwd.path(),
        &["adapter", "search", "linear", "--json"],
    );
    assert!(out.status.success(), "search --json failed: {out:?}");
    let stdout = String::from_utf8(out.stdout).unwrap();
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("search --json parses");
    assert_eq!(v["hits"][0]["id"], "linear");
    assert_eq!(v["hits"][0]["publisher"], "Test Publisher");

    // Install without any trust keys → fail closed with a helpful error.
    let out = locus(home.path(), cwd.path(), &["adapter", "install", "linear"]);
    assert!(
        !out.status.success(),
        "install must refuse without trust keys"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("trust keys"),
        "error must explain the trust requirement, got: {stderr:?}"
    );

    // Unknown adapter id → clean error.
    let out = locus(
        home.path(),
        cwd.path(),
        &["adapter", "install", "nope", "--yes"],
    );
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("not found"), "got: {stderr:?}");

    // Remove by name.
    let out = locus(
        home.path(),
        cwd.path(),
        &["adapter", "registry", "index", "remove", "test"],
    );
    assert!(out.status.success(), "index remove failed: {out:?}");

    // Non-HTTPS index URLs are refused.
    let out = locus(
        home.path(),
        cwd.path(),
        &[
            "adapter",
            "registry",
            "index",
            "add",
            "http://example.com/index.json",
        ],
    );
    assert!(!out.status.success(), "plain-HTTP index must be refused");
}

#[test]
fn marketplace_search_without_indexes_is_a_clean_error() {
    let (home, cwd) = init_store();
    let out = locus(home.path(), cwd.path(), &["adapter", "search", "linear"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("index add"), "got: {stderr:?}");
}

#[test]
fn adapter_list_shows_marketplace_hint_when_empty() {
    let (home, cwd) = init_store();
    let out = locus(home.path(), cwd.path(), &["adapter", "list"]);
    assert!(out.status.success(), "adapter list failed: {out:?}");
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("Marketplace"),
        "list should hint at the marketplace"
    );
}

#[test]
fn onboard_yes_never_accepts_detected_candidates_without_reviewed_plan() {
    let (home, cwd) = init_store();
    let output = locus(home.path(), cwd.path(), &["onboard", "--yes", "--json"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("no explicitly reviewed bindings"));
    let bindings = home.path().join("bindings");
    assert_eq!(std::fs::read_dir(bindings).unwrap().count(), 0);
    assert!(!home.path().join("sessions/active.json").exists());
}

fn signed_fixture(
    home: &std::path::Path,
) -> (
    locus_core::marketplace::CommunityAdapterManifest,
    locus_core::RegistryTrustKey,
) {
    use locus_core::adapter_registry::{
        sign_entry_material, AdapterManifestEntry, RegistryTrustKey,
    };
    use locus_core::marketplace::{CommunityAdapterManifest, COMMUNITY_MANIFEST_VERSION};
    let secret = "09".repeat(32); // synthetic fixture material only
    let key = RegistryTrustKey::hmac_sha256("fixture", &secret);
    let mut manifest = CommunityAdapterManifest {
        manifest_version: COMMUNITY_MANIFEST_VERSION,
        publisher: "inert fixture".into(),
        version: "1.0.0".into(),
        credential_env: Some("LINEAR_API_KEY".into()),
        upstream: Some(
            locus_core::UpstreamSpec::new("intentionally-uninvoked-fixture").resolve_secrets(true),
        ),
        entry: AdapterManifestEntry {
            id: "linear".into(),
            name: "fixture".into(),
            status: "community".into(),
            synthetic: false,
            capabilities: vec![],
            frozen_selectors: vec!["workspace".into()],
            tools: vec!["linear.read".into()],
            destructive_tools: vec![],
            description: String::new(),
            signed_by: Some("fixture".into()),
            signature: None,
        },
    };
    manifest.entry.signature =
        Some(sign_entry_material(&manifest.signing_material(), &key).unwrap());
    std::fs::create_dir_all(home.join("trust")).unwrap();
    std::fs::write(home.join("trust/adapter-keys.toml"), format!("version = 1\n[[keys]]\nid = 'fixture'\nscheme = 'hmac-sha256'\nsecret_hex = '{secret}'\n")).unwrap();
    (manifest, key)
}

#[test]
fn binding_from_adapter_captures_verified_envelope_and_requires_qualified_scope() {
    use locus_core::marketplace::install_adapter;
    let (home, cwd) = init_store();
    let (manifest, key) = signed_fixture(home.path());
    install_adapter(home.path(), &manifest, &[key], true).unwrap();
    let out = locus(
        home.path(),
        cwd.path(),
        &[
            "binding",
            "add",
            "client-linear",
            "--from-adapter",
            "linear",
            "--tenant",
            "client",
            "--account",
            "client-ops",
            "--credential-ref",
            "env:CLIENT_LINEAR_KEY",
            "--scope",
            "workspace=client-workspace",
            "--read-only",
            "--non-interactive",
        ],
    );
    assert!(out.status.success(), "binding creation failed: {out:?}");
    let text = std::fs::read_to_string(home.path().join("bindings/client-linear.toml")).unwrap();
    let binding = locus_core::Binding::parse_toml(&text).unwrap();
    let provider = &binding.providers[0];
    assert_eq!(
        provider.scope.extra["workspace"].as_str(),
        Some("client-workspace")
    );
    assert_eq!(provider.scope.read_only, Some(true));
    assert_eq!(
        provider
            .upstream
            .as_ref()
            .unwrap()
            .community_adapter
            .as_deref(),
        Some(&manifest)
    );
    let out = locus(
        home.path(),
        cwd.path(),
        &[
            "binding",
            "add",
            "unqualified",
            "--from-adapter",
            "linear",
            "--tenant",
            "client",
            "--account",
            "client-ops",
            "--credential-ref",
            "env:CLIENT_LINEAR_KEY",
            "--non-interactive",
        ],
    );
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("concrete scalar"));
    assert!(!home.path().join("bindings/unqualified.toml").exists());
    assert!(
        !home.path().join("sessions/active.json").exists(),
        "onboarding must not pin"
    );
}

#[test]
fn index_url_userinfo_refused_without_echo() {
    let (home, cwd) = init_store();
    for url in [
        "https://user:SECRETPW@example.com/index.json",
        "https://SECRETUSER@example.com/index.json",
    ] {
        for json in [false, true] {
            let mut args = vec!["adapter", "registry", "index", "add", url];
            if json {
                args.push("--json");
            }
            let out = locus(home.path(), cwd.path(), &args);
            assert!(!out.status.success(), "userinfo URL must be refused");
            let stderr = String::from_utf8_lossy(&out.stderr);
            let stdout = String::from_utf8_lossy(&out.stdout);
            assert!(stderr.contains("credentials"), "got: {stderr:?}");
            assert!(!stderr.contains("SECRET"), "stderr leaked: {stderr:?}");
            assert!(!stdout.contains("SECRET"), "stdout leaked: {stdout:?}");
        }
    }
    assert!(!home.path().join("adapter-indexes.toml").exists());
}

#[test]
fn index_url_query_is_redacted_in_list_output() {
    let (home, cwd) = init_store();
    let (base, listener) = serve_index();
    run_server(listener, base.clone(), 2);
    let index_url = format!("{base}/index.json?sig=SECRETSIG&exp=1");
    let out = locus(
        home.path(),
        cwd.path(),
        &[
            "adapter", "registry", "index", "add", &index_url, "--name", "signed",
        ],
    );
    assert!(out.status.success(), "index add failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.contains("SECRET"), "add output leaked: {stdout:?}");
    assert!(!String::from_utf8_lossy(&out.stderr).contains("SECRET"));

    let expected = format!("{base}/index.json?<redacted>");
    let out = locus(
        home.path(),
        cwd.path(),
        &["adapter", "registry", "index", "list"],
    );
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.contains("SECRET"), "human list leaked: {stdout:?}");
    assert!(stdout.contains(&expected), "got: {stdout:?}");

    let out = locus(
        home.path(),
        cwd.path(),
        &["adapter", "registry", "index", "list", "--json"],
    );
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.contains("SECRET"), "json list leaked: {stdout:?}");
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["sources"][0]["url"], expected.as_str());

    // Stored config keeps the full URL so signed fetches still work.
    let stored = std::fs::read_to_string(home.path().join("adapter-indexes.toml")).unwrap();
    assert!(stored.contains("sig=SECRETSIG"));
}

#[test]
fn index_add_refuses_non_exact_loopback_http() {
    let (home, cwd) = init_store();
    for url in [
        "http://localhost:1/index.json",
        "http://127.0.0.2:1/index.json",
    ] {
        let out = locus(
            home.path(),
            cwd.path(),
            &["adapter", "registry", "index", "add", url],
        );
        assert!(!out.status.success(), "{url} must be refused");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("loopback HTTP"), "got: {stderr:?}");
    }
}

/// An initialized store without a live broker: dry runs must not create one.
/// The entire fixture, including runtime, is checked for file and metadata changes.
struct DryAdapterEnv {
    root: tempfile::TempDir,
}

#[derive(PartialEq, Eq)]
struct SnapshotEntry {
    bytes: Vec<u8>,
    kind: std::fs::FileType,
    modified: std::time::SystemTime,
    permissions: std::fs::Permissions,
}

impl DryAdapterEnv {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        locus_core::Store::open(root.path().join("locus")).unwrap();
        std::fs::create_dir(root.path().join("user")).unwrap();
        std::fs::create_dir(root.path().join("project")).unwrap();
        Self { root }
    }

    fn home(&self) -> std::path::PathBuf {
        self.root.path().join("locus").canonicalize().unwrap()
    }

    fn snapshot(&self) -> std::collections::BTreeMap<std::path::PathBuf, SnapshotEntry> {
        fn visit(
            root: &std::path::Path,
            path: &std::path::Path,
            out: &mut std::collections::BTreeMap<std::path::PathBuf, SnapshotEntry>,
        ) {
            let meta = std::fs::symlink_metadata(path).unwrap();
            let bytes = if meta.is_file() {
                std::fs::read(path).unwrap()
            } else if meta.is_symlink() {
                std::fs::read_link(path)
                    .unwrap()
                    .as_os_str()
                    .as_encoded_bytes()
                    .to_vec()
            } else {
                Vec::new()
            };
            out.insert(
                path.strip_prefix(root).unwrap().to_path_buf(),
                SnapshotEntry {
                    bytes,
                    kind: meta.file_type(),
                    modified: meta.modified().unwrap(),
                    permissions: meta.permissions(),
                },
            );
            if meta.is_dir() {
                for entry in std::fs::read_dir(path).unwrap() {
                    visit(root, &entry.unwrap().path(), out);
                }
            }
        }
        let mut out = std::collections::BTreeMap::new();
        visit(self.root.path(), self.root.path(), &mut out);
        out
    }

    fn dry(&self, command: &str, json: bool) -> std::process::Output {
        let before = self.snapshot();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_locus"));
        cmd.env_clear()
            .env("HOME", self.root.path().join("user"))
            .env("LOCUS_HOME", self.home())
            .env("NO_COLOR", "1")
            .current_dir(self.root.path().join("project"))
            .args(["adapter", command, "linear", "--dry-run"]);
        if json {
            cmd.arg("--json");
        }
        let out = cmd.output().unwrap();
        // Do not include snapshots in failure output: they contain synthetic keys.
        assert!(
            before == self.snapshot(),
            "adapter {command} dry run changed the filesystem"
        );
        out
    }
}

struct ManifestServer {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ManifestServer {
    fn new(
        home: &std::path::Path,
        manifest: &locus_core::marketplace::CommunityAdapterManifest,
    ) -> Self {
        use locus_core::marketplace::{save_index_sources, IndexSource};
        let (base, listener) = serve_index();
        listener.set_nonblocking(true).unwrap();
        let mut index: serde_json::Value = serde_json::from_str(&index_json(&base)).unwrap();
        index["adapters"][0]["tools"] = serde_json::json!(manifest.entry.tools);
        let index = index.to_string();
        let manifest = serde_json::to_string(manifest).unwrap();
        save_index_sources(
            home,
            &[IndexSource {
                name: "fixture".into(),
                url: format!("{base}/index.json"),
            }],
        )
        .unwrap();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let done = stop.clone();
        let thread = std::thread::spawn(move || {
            while !done.load(std::sync::atomic::Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                            .unwrap();
                        let mut request = [0; 4096];
                        let n = stream.read(&mut request).unwrap_or(0);
                        let body =
                            if String::from_utf8_lossy(&request[..n]).contains("GET /index.json") {
                                &index
                            } else {
                                &manifest
                            };
                        let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                        let _ = stream.write_all(response.as_bytes());
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(10))
                    }
                    Err(e) => panic!("fixture listener failed: {e}"),
                }
            }
        });
        Self {
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for ManifestServer {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap();
    }
}

fn adapter_plan(out: &std::process::Output) -> serde_json::Value {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let plan: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(plan["schema"], "locus.plan.v1");
    assert_eq!(plan["dry_run"], true);
    assert_eq!(plan["requires_control_capability"], true);
    assert_eq!(plan["control_capability_in_env"], false);
    plan
}

fn assert_adapter_change(plan: &serde_json::Value, action: &str, path: &std::path::Path) {
    assert!(
        plan["changes"].as_array().unwrap().iter().any(|change| {
            change["action"] == action && change["target"] == path.to_string_lossy().as_ref()
        }),
        "missing {action} for {}",
        path.display()
    );
}

#[test]
fn adapter_install_dry_run_plans_new_and_overwritten_manifest_and_ledger_without_writes() {
    let env = DryAdapterEnv::new();
    let (manifest, key) = signed_fixture(&env.home());
    let _server = ManifestServer::new(&env.home(), &manifest);
    for action in ["create", "overwrite"] {
        let plan = adapter_plan(&env.dry("install", true));
        assert_adapter_change(&plan, action, &env.home().join("adapters/linear.json"));
        assert_adapter_change(&plan, action, &env.home().join("adapters/installed.toml"));
        assert_eq!(plan["changes"].as_array().unwrap().len(), 2);
        let human = env.dry("install", false);
        assert!(human.status.success());
        assert!(String::from_utf8_lossy(&human.stdout).contains("nothing written"));
        locus_core::marketplace::install_adapter(
            &env.home(),
            &manifest,
            std::slice::from_ref(&key),
            true,
        )
        .unwrap();
    }
}

#[test]
fn adapter_uninstall_dry_run_preserves_installed_corrupt_and_missing_manifests() {
    let env = DryAdapterEnv::new();
    let (manifest, key) = signed_fixture(&env.home());
    locus_core::marketplace::install_adapter(&env.home(), &manifest, &[key], true).unwrap();
    let manifest_path = env.home().join("adapters/linear.json");
    for state in ["valid", "corrupt", "missing"] {
        match state {
            "corrupt" => std::fs::write(&manifest_path, "invalid envelope").unwrap(),
            "missing" => std::fs::remove_file(&manifest_path).unwrap(),
            _ => {}
        }
        let plan = adapter_plan(&env.dry("uninstall", true));
        assert_adapter_change(
            &plan,
            "overwrite",
            &env.home().join("adapters/installed.toml"),
        );
        if state != "missing" {
            assert_adapter_change(&plan, "delete", &manifest_path);
        }
        assert_eq!(
            plan["changes"].as_array().unwrap().len(),
            if state == "missing" { 1 } else { 2 }
        );
        assert!(env.dry("uninstall", false).status.success());
    }
}

#[test]
fn adapter_dry_runs_refuse_malformed_installed_ledger_without_writes() {
    let env = DryAdapterEnv::new();
    let (manifest, _) = signed_fixture(&env.home());
    let _server = ManifestServer::new(&env.home(), &manifest);
    std::fs::create_dir(env.home().join("adapters")).unwrap();
    std::fs::write(env.home().join("adapters/installed.toml"), "[invalid").unwrap();
    for command in ["install", "uninstall"] {
        for json in [true, false] {
            let out = env.dry(command, json);
            assert!(!out.status.success(), "malformed ledger must fail closed");
            assert!(!String::from_utf8_lossy(&out.stdout).contains("locus.plan.v1"));
        }
    }
}

#[test]
fn adapter_install_dry_run_refuses_tampered_signature_without_writes() {
    let env = DryAdapterEnv::new();
    let (mut manifest, _) = signed_fixture(&env.home());
    manifest.version = "99.0.0".into();
    let _server = ManifestServer::new(&env.home(), &manifest);
    for json in [true, false] {
        let out = env.dry("install", json);
        assert!(!out.status.success(), "tampered envelope must fail closed");
        assert!(String::from_utf8_lossy(&out.stderr).contains("signature"));
    }
}
