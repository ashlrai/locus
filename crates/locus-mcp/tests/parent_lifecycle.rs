#![cfg(unix)]

//! Process-lifecycle contract: a client-launched `locus-mcp` must not outlive
//! the process that launched it. Exit paths under test:
//!
//! - **stdin EOF** — the client held the stdin write end and died (or simply
//!   closed the pipe): the read loop ends and the server exits.
//! - **Parent death without EOF** — stdin is shared (`locus mcp` and the npm
//!   wrapper launch with inherited stdio), so a killed middle process leaves
//!   a survivor holding the write end and no EOF ever arrives. The server's
//!   parent watchdog (kqueue `NOTE_EXIT` on macOS, `getppid()` poll
//!   elsewhere) must end it instead.

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::os::fd::{FromRawFd, RawFd};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const MARKER_ENV: &str = "LOCUS_LIFECYCLE_MARKER";
/// Read end of a pipe the driver keeps open — the marker-mode "parent" gives
/// it to the server as stdin so no EOF can arrive after the parent dies.
const STDIN_FD_ENV: &str = "LOCUS_LIFECYCLE_STDIN_FD";
/// File path the marker-mode parent points the server's stderr at, both to
/// wait for "watchdog armed" (kills cannot race startup) and to capture the
/// shutdown reason afterwards.
const STDERR_LOG_ENV: &str = "LOCUS_LIFECYCLE_STDERR_LOG";

struct ServerGuard(Child);

impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The marker-mode helper is this same test binary re-executed; the
/// `--exact` arg selects which test (and therefore which mode) it enters.
fn respawn_self(test: &str) -> Command {
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", test, "--nocapture"]);
    cmd
}

/// Wait for the marker file a marker-mode helper writes once its server is
/// up, then return the recorded server pid.
fn wait_for_server_pid(marker: &Path, helper: &mut Child) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !marker.exists() {
        assert!(
            Instant::now() < deadline,
            "server did not start (marker never appeared)"
        );
        assert!(
            helper.try_wait().unwrap().is_none(),
            "helper exited before server was ready"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let recorded = std::fs::read_to_string(marker).unwrap();
    recorded.split('|').next().unwrap().parse().unwrap()
}

/// Poll until `pid` is dead-or-a-zombie; on deadline kill it and panic.
fn assert_gone_within(pid: u32, budget: Duration, context: &str) {
    let deadline = Instant::now() + budget;
    loop {
        // A zombie has exited and is only awaiting its new parent's reap.
        #[cfg(target_os = "linux")]
        let zombie = std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|stat| {
                stat.rsplit_once(')')
                    .map(|(_, rest)| rest.trim_start().starts_with('Z'))
            })
            .unwrap_or(false);
        #[cfg(not(target_os = "linux"))]
        let zombie = false;
        // Signal 0 checks for the process without changing it.
        let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
        let gone =
            result != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
        if gone || zombie {
            return;
        }
        if Instant::now() >= deadline {
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGKILL);
            }
            panic!("{context}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn mcp_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_locus-mcp"))
}

fn clean_env(cmd: &mut Command, home: &Path) {
    cmd.env("HOME", home)
        .env("LOCUS_HOME", home.join("store"))
        .env_remove("LOCUS_EXECUTOR_CAPABILITY")
        .env_remove("LOCUS_CONTROL_CAPABILITY");
}

// ─── stdin EOF ───────────────────────────────────────────────────────────

/// Closing the server's stdin (the pipe write end drops) ends the server
/// within the lifecycle budget — no parent death needed.
#[test]
fn server_exits_on_stdin_eof() {
    let home = tempfile::tempdir().unwrap();
    let mut server = ServerGuard({
        let mut cmd = Command::new(mcp_bin());
        clean_env(&mut cmd, home.path());
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
    });
    {
        let mut stdin = server.0.stdin.take().unwrap();
        writeln!(stdin, "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{{\"protocolVersion\":\"2024-11-05\",\"capabilities\":{{}},\"clientInfo\":{{\"name\":\"test\",\"version\":\"1\"}}}}}}").unwrap();
        stdin.flush().unwrap();
        // drop(stdin): the last write end closes — EOF.
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(status) = server.0.try_wait().unwrap() {
            assert!(status.success(), "EOF exit should be clean: {status}");
            return;
        }
        assert!(
            Instant::now() < deadline,
            "server did not exit on stdin EOF"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

// The helper owns the only write end of the server's stdin. Killing it models
// a client crash while a native server is actively serving requests.
#[test]
fn native_server_exits_after_client_is_killed_ten_times() {
    if let Some(marker) = std::env::var_os(MARKER_ENV) {
        let mut server = ServerGuard(
            Command::new(mcp_bin())
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let mut stdin = server.0.stdin.take().unwrap();
        let mut stdout = BufReader::new(server.0.stdout.take().unwrap());
        writeln!(stdin, "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{{\"protocolVersion\":\"2024-11-05\",\"capabilities\":{{}},\"clientInfo\":{{\"name\":\"test\",\"version\":\"1\"}}}}}}").unwrap();
        stdin.flush().unwrap();
        let mut reply = String::new();
        stdout.read_line(&mut reply).unwrap();
        assert!(reply.contains("serverInfo"));
        std::fs::write(marker, server.0.id().to_string()).unwrap();
        loop {
            std::thread::park();
        }
    }
    let home = tempfile::tempdir().unwrap();
    for iteration in 0..10 {
        let marker = home.path().join(format!("child-{iteration}"));
        let mut cmd = respawn_self("native_server_exits_after_client_is_killed_ten_times");
        cmd.env(MARKER_ENV, &marker)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        clean_env(&mut cmd, home.path());
        let mut client = cmd.spawn().unwrap();
        let pid = wait_for_server_pid(&marker, &mut client);
        client.kill().unwrap();
        client.wait().unwrap();
        assert_gone_within(
            pid,
            Duration::from_secs(5),
            &format!("server survived client death in iteration {iteration}"),
        );
    }
}

// ─── Parent death without stdin EOF ─────────────────────────────────────

/// Marker-mode entry: this process is the "parent" the driver kills. It
/// gives the server a stdin whose write end a surviving process (the driver)
/// keeps open, waits until the server reports its watchdog armed, records
/// the pid, then parks until killed.
fn parent_marker_mode() -> ! {
    let marker = std::env::var_os(MARKER_ENV).unwrap();
    let fd: RawFd = std::env::var(STDIN_FD_ENV)
        .unwrap()
        .parse()
        .expect("stdin fd");
    let log_path: PathBuf = std::env::var_os(STDERR_LOG_ENV).unwrap().into();
    let log = File::create(&log_path).unwrap();
    let mut server = ServerGuard(
        Command::new(mcp_bin())
            // SAFETY: `fd` is a live pipe read end inherited from the driver.
            .stdin(unsafe { Stdio::from_raw_fd(fd) })
            .stdout(Stdio::null())
            .stderr(Stdio::from(log))
            .spawn()
            .unwrap(),
    );
    // The watchdog records its parent pid while arming; wait for the armed
    // line so the driver's kill cannot race startup. On timeout, still write
    // the marker: the driver's kill + survival check then reports the real
    // regression ("server outlived its parent") instead of a startup timeout.
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut armed = false;
    while Instant::now() < deadline {
        if let Ok(text) = std::fs::read_to_string(&log_path) {
            if text.contains("watchdog armed") {
                armed = true;
                break;
            }
        }
        if server.0.try_wait().unwrap().is_some() {
            panic!("server exited before the watchdog armed");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    std::fs::write(
        marker,
        format!(
            "{}|{}",
            server.0.id(),
            if armed { "armed" } else { "unarmed" }
        ),
    )
    .unwrap();
    loop {
        std::thread::park();
    }
}

/// The server must exit within 2s of its parent dying even when stdin never
/// delivers EOF — the case `locus mcp` (inherited stdio) leaks today.
#[test]
fn server_exits_when_parent_dies_while_stdin_stays_open() {
    if std::env::var_os(MARKER_ENV).is_some() {
        parent_marker_mode();
    }
    let home = tempfile::tempdir().unwrap();
    let marker = home.path().join("parent-death-child");
    let log = home.path().join("parent-death.stderr");

    // Pipe whose write end stays open in THIS process for the whole test:
    // after the parent dies the server's stdin still cannot EOF.
    let mut fds = [0 as RawFd; 2];
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0, "pipe");
    let (read_fd, write_fd) = (fds[0], fds[1]);

    let mut cmd = respawn_self("server_exits_when_parent_dies_while_stdin_stays_open");
    cmd.env(MARKER_ENV, &marker)
        .env(STDIN_FD_ENV, read_fd.to_string())
        .env(STDERR_LOG_ENV, &log)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    clean_env(&mut cmd, home.path());
    let mut parent = cmd.spawn().unwrap();
    // The parent's copy (and the server's stdin) own the read end now.
    assert_eq!(unsafe { libc::close(read_fd) }, 0);

    let pid = wait_for_server_pid(&marker, &mut parent);
    let recorded = std::fs::read_to_string(&marker).unwrap();
    assert!(
        recorded.ends_with("|armed"),
        "watchdog never armed before the kill: {recorded}"
    );
    parent.kill().unwrap();
    parent.wait().unwrap();

    assert_gone_within(pid, Duration::from_secs(2), "server outlived its parent");
    assert_eq!(unsafe { libc::close(write_fd) }, 0);

    let text = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(
        text.contains("parent process") && text.contains("exited"),
        "expected the watchdog shutdown line in stderr: {text}"
    );
}
