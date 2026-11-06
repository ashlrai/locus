#![cfg(unix)]

use locus_core::workers::{McpStdioBackend, McpStdioConfig, ProcessGuard, WorkerBackend};
use locus_core::{Binding, PinSource, SealKey, Session};
use std::process::{Command, Stdio};

fn assert_reaped(pid: u32) {
    // WNOHANG distinguishes a running child (0), an unreaped exit (pid),
    // and a child already reaped by the guard (-1/ECHILD).
    // SAFETY: pid belongs to this test's child; the null status pointer is allowed.
    let result = unsafe { libc::waitpid(pid as libc::pid_t, std::ptr::null_mut(), libc::WNOHANG) };
    let error = std::io::Error::last_os_error();
    assert_eq!(result, -1, "child {pid} still running or unreaped");
    assert_eq!(error.raw_os_error(), Some(libc::ECHILD));
}

#[test]
fn drop_kills_and_reaps_running_child() {
    let child = Command::new("/bin/sleep")
        .arg("30")
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let pid = child.id();
    let mut guard = ProcessGuard::new(child);
    assert!(guard.child_mut().try_wait().unwrap().is_none());
    let start = std::time::Instant::now();
    drop(guard);
    assert!(start.elapsed() < std::time::Duration::from_secs(5));
    assert_reaped(pid);
}

#[test]
fn drop_accepts_already_reaped_child() {
    let child = Command::new("/usr/bin/true").env_clear().spawn().unwrap();
    let pid = child.id();
    let mut guard = ProcessGuard::new(child);
    assert!(guard.child_mut().wait().unwrap().success());
    drop(guard);
    assert_reaped(pid);
}

fn backend_fixture(script: &str) -> (tempfile::TempDir, Session, Binding, McpStdioBackend) {
    let home = tempfile::tempdir().unwrap();
    let binding = Binding::parse_toml(
        "id = 'bnd_test'\nalias = 'test'\ntenant = 'test'\n\
         [[providers]]\nprovider = 'github'\naccount = 'test'\ncredential_ref = 'env:UNUSED'",
    )
    .unwrap();
    let session = Session::new(
        &binding.id,
        &binding.alias,
        &binding.tenant,
        None,
        PinSource::Explicit,
        None,
        chrono::Duration::hours(1),
        home.path().display().to_string(),
        &SealKey::generate(),
    );
    let backend = McpStdioBackend::new(McpStdioConfig {
        command: "/bin/sh".into(),
        args: vec!["-c".into(), script.into()],
        spawn: true,
        ..Default::default()
    });
    (home, session, binding, backend)
}

#[test]
fn dropping_stdio_backend_kills_and_reaps_worker() {
    let (home, session, binding, backend) = backend_fixture(
        r#"
while IFS= read -r line; do
    case "$line" in
        *'"method":"initialize"'*) printf '%s\n' '{"id":1,"result":{}}';;
        *'"method":"tools/list"'*) printf '%s\n' '{"id":2,"result":{"tools":[]}}';;
    esac
done
exec /bin/sleep 30
"#,
    );
    let slot = backend
        .ensure(&session, &binding, &binding.providers[0], home.path())
        .unwrap();
    drop(backend);
    assert_reaped(slot.pid.unwrap());
}

#[test]
fn failed_handshake_kills_and_reaps_worker() {
    let (home, session, binding, backend) =
        backend_fixture("echo $$ > worker.pid; printf 'invalid-json\\n'; exec /bin/sleep 30");
    assert!(backend
        .ensure(&session, &binding, &binding.providers[0], home.path())
        .is_err());
    let pid = std::fs::read_to_string(home.path().join("worker.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_reaped(pid);
}
