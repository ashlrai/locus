//! Real process-tree regression tests. Every backend lives in a disposable
//! parent process so SIGKILL exercises cleanup without Rust destructors.
#![cfg(unix)]

use locus_core::seal::SealKey;
use locus_core::session::{PinSource, Session};
use locus_core::workers::{McpStdioBackend, McpStdioConfig, WorkerBackend};
use locus_core::{Binding, BindingBody, Policy, ProviderBinding, Scope};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const CHILD_CASE: &str = "LOCUS_TEST_WORKER_LIFECYCLE";
const SERVER: &str = r#"
import json, os, pathlib, subprocess, sys, time
root = pathlib.Path(sys.argv[1])
mode = sys.argv[2]
descendant = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(60)'],
    stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
(root / 'pids').write_text(str(os.getpid()) + '\n' + str(descendant.pid))
if mode == 'failed_handshake':
    print('malformed MCP response', flush=True)
    time.sleep(60)
else:
    for line in sys.stdin:
        request = json.loads(line)
        if 'id' not in request:
            continue
        result = {'tools': []} if request['method'] == 'tools/list' else {
            'protocolVersion': '2024-11-05', 'capabilities': {},
            'serverInfo': {'name': 'lifecycle-fixture', 'version': '0'}}
        print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)
    (root / 'eof').write_text('stdin closed')
    if mode != 'cooperative':
        time.sleep(60)
"#;

fn fixture_parent(root: &Path, mode: &str) {
    let worker_home = root.join("worker");
    std::fs::create_dir_all(&worker_home).unwrap();
    let binding = Binding::from_body(BindingBody {
        id: "bnd_lifecycle".into(),
        alias: "lifecycle".into(),
        tenant: "synthetic".into(),
        principal: None,
        description: None,
        policy: Policy::default(),
        providers: vec![ProviderBinding {
            provider: "github".into(),
            account: "synthetic".into(),
            credential_ref: "env:LOCUS_UNUSED_LIFECYCLE_CREDENTIAL".into(),
            scope: Scope::default(),
            upstream: None,
        }],
    });
    let session = Session::new(
        &binding.id,
        &binding.alias,
        &binding.tenant,
        None,
        PinSource::Explicit,
        None,
        chrono::Duration::hours(1),
        worker_home.display().to_string(),
        &SealKey::generate(),
    );
    let backend = McpStdioBackend::new(McpStdioConfig {
        command: "python3".into(),
        args: vec![
            "-u".into(),
            "-c".into(),
            SERVER.into(),
            root.display().to_string(),
            mode.into(),
        ],
        spawn: true,
        ..McpStdioConfig::default()
    });
    let result = backend.ensure(
        &session,
        &binding,
        &binding.providers[0],
        &worker_home.join("slots/github"),
    );
    if mode == "failed_handshake" {
        assert!(result.is_err(), "malformed handshake must fail closed");
        return;
    }
    let slot = result.unwrap();
    std::fs::write(root.join("ready"), "handshake complete").unwrap();
    match mode {
        "killed_parent" | "cooperative" => loop {
            std::thread::sleep(Duration::from_secs(1));
        },
        "teardown" => backend.teardown(&slot).unwrap(),
        "drop" => drop(backend),
        _ => panic!("unknown lifecycle fixture mode"),
    }
}

/// Cleanup even when an assertion fails, so a regression cannot leave a new
/// process leak on the machine running the tests.
struct Fixture {
    parent: Child,
    root: tempfile::TempDir,
    pids: Vec<i32>,
    cleanup_armed: bool,
}

impl Fixture {
    fn read_pids(&mut self) {
        if let Ok(text) = std::fs::read_to_string(self.root.path().join("pids")) {
            self.pids = text.lines().map(|line| line.parse().unwrap()).collect();
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.parent.kill();
        let _ = self.parent.wait();
        if !self.cleanup_armed {
            return;
        }
        self.read_pids();
        if let Some(worker) = self.pids.first() {
            // The backend gives the worker its own process group.
            unsafe { libc::kill(-*worker, libc::SIGKILL) };
        }
        for pid in &self.pids {
            unsafe { libc::kill(*pid, libc::SIGKILL) };
        }
    }
}

fn wait_until(description: &str, mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !predicate() {
        assert!(Instant::now() < deadline, "timed out: {description}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn process_running(pid: i32) -> bool {
    if unsafe { libc::kill(pid, 0) } == -1
        && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    {
        return false;
    }
    // Orphan zombies are already dead; only PID 1 can reap them.
    #[cfg(target_os = "linux")]
    if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        if stat
            .rsplit_once(") ")
            .is_some_and(|(_, tail)| tail.starts_with('Z'))
        {
            return false;
        }
    }
    #[cfg(target_os = "macos")]
    unsafe {
        let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
        if libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        ) == size
            && info.assume_init().pbi_status == libc::SZOMB
        {
            return false;
        }
    }
    true
}

fn run_case(test_name: &str, mode: &str) {
    if std::env::var(CHILD_CASE).ok().as_deref() == Some(test_name) {
        let root = std::env::var_os("LOCUS_HOME").unwrap();
        fixture_parent(Path::new(&root), mode);
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.args([test_name, "--exact", "--nocapture", "--test-threads=1"]);
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("LOCUS_") {
            command.env_remove(key);
        }
    }
    command
        .env(CHILD_CASE, test_name)
        .env("HOME", root.path())
        .env("LOCUS_HOME", root.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    let mut fixture = Fixture {
        parent: command.spawn().unwrap(),
        root,
        pids: Vec::new(),
        cleanup_armed: true,
    };
    let abrupt = matches!(mode, "killed_parent" | "cooperative");
    if abrupt {
        wait_until("parent completed MCP handshake", || {
            assert!(
                fixture.parent.try_wait().unwrap().is_none(),
                "fixture parent exited before completing handshake"
            );
            fixture.root.path().join("ready").exists()
        });
        fixture.parent.kill().unwrap(); // SIGKILL: no Drop or signal cleanup.
        fixture.parent.wait().unwrap();
    } else {
        wait_until("parent exits after cleanup", || {
            if let Some(status) = fixture.parent.try_wait().unwrap() {
                assert!(status.success(), "fixture parent failed: {status}");
                true
            } else {
                false
            }
        });
    }
    fixture.read_pids();
    assert_eq!(fixture.pids.len(), 2, "fixture did not publish worker tree");
    wait_until("worker and descendant terminate", || {
        fixture.pids.iter().all(|pid| !process_running(*pid))
    });
    if mode != "failed_handshake" {
        assert!(
            fixture.root.path().join("eof").exists(),
            "cooperative shutdown opportunity must close worker stdin before killing it"
        );
    }
    fixture.cleanup_armed = false;
}

#[test]
fn killed_parent_leaves_no_worker_or_descendant() {
    run_case(
        "killed_parent_leaves_no_worker_or_descendant",
        "killed_parent",
    );
}

#[test]
fn parent_death_delivers_stdin_eof_to_cooperative_server() {
    run_case(
        "parent_death_delivers_stdin_eof_to_cooperative_server",
        "cooperative",
    );
}

#[test]
fn backend_drop_closes_stdin_and_reaps_process_tree() {
    run_case("backend_drop_closes_stdin_and_reaps_process_tree", "drop");
}

#[test]
fn teardown_closes_stdin_and_reaps_process_tree() {
    run_case("teardown_closes_stdin_and_reaps_process_tree", "teardown");
}

#[test]
fn failed_handshake_reaps_worker_and_descendant() {
    run_case(
        "failed_handshake_reaps_worker_and_descendant",
        "failed_handshake",
    );
}
