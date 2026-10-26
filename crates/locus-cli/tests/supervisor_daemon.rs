//! Durable supervision is opt-in. Shell exit must preserve the live authority
//! generation; supervisor death and restart must never resurrect an old pin.
#![cfg(unix)]

use serde::Deserialize;
use serde_json::Value;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

struct Fixture {
    root: tempfile::TempDir,
    home: PathBuf,
    cwd: PathBuf,
    capability: String,
}

#[derive(Deserialize)]
struct Ready {
    ready: bool,
    supervisor_pid: u32,
    broker_pid: u32,
    epoch: String,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("locus");
        let cwd = root.path().join("workspace");
        fs::create_dir(&cwd).unwrap();
        fs::create_dir(root.path().join("user")).unwrap();
        let fixture = Self {
            root,
            home,
            cwd,
            capability: "a1".repeat(32),
        };
        // Keep a real shell parent through init, then let that shell and its
        // broker exit before asking the durable supervisor to own this home.
        let output = fixture
            .shell("\"$0\" init --with-samples && \"$0\" capability persist; exit $?")
            .output()
            .unwrap();
        fixture.success(&output, "initialize isolated store");
        wait_until("initial shell broker retirement", || {
            !fixture.endpoint_path().exists()
        });
        fixture
    }

    fn configure(&self, command: &mut Command) {
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.root.path().join("user"))
            .env("LOCUS_HOME", &self.home)
            .env("LOCUS_CONTROL_CAPABILITY", &self.capability)
            .env("NO_COLOR", "1")
            .env("SHELL", "/bin/sh")
            .current_dir(&self.cwd)
            .stdin(Stdio::null());
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_locus"));
        self.configure(&mut command);
        command.args(args);
        command
    }

    fn shell(&self, script: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        self.configure(&mut command);
        command
            .args(["-c", script])
            .arg(env!("CARGO_BIN_EXE_locus"));
        command
    }

    fn output(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    fn no_bearer(&self, output: &Output) {
        for bytes in [&output.stdout, &output.stderr] {
            assert!(
                !String::from_utf8_lossy(bytes).contains(&self.capability),
                "command disclosed its synthetic control capability"
            );
        }
    }

    fn success(&self, output: &Output, operation: &str) {
        self.no_bearer(output);
        // Do not include arbitrary CLI output in assertion diagnostics: these
        // tests must remain safe if a future regression prints a capability.
        assert!(output.status.success(), "{operation} failed");
    }

    fn endpoint_path(&self) -> PathBuf {
        self.home.join("runtime/authority.json")
    }

    fn endpoint(&self) -> Value {
        serde_json::from_slice(&fs::read(self.endpoint_path()).unwrap()).unwrap()
    }

    fn start(&self) -> Ready {
        let output = self.output(&["--json", "supervisor", "start"]);
        self.success(&output, "start durable supervisor");
        let ready: Ready = serde_json::from_slice(&output.stdout).unwrap();
        self.verify_ready(&ready);
        ready
    }

    fn verify_ready(&self, ready: &Ready) {
        assert!(ready.ready);
        assert_ne!(ready.supervisor_pid, ready.broker_pid);
        let endpoint = self.endpoint();
        assert_eq!(endpoint["supervisor"]["pid"], ready.supervisor_pid);
        assert_eq!(endpoint["owner_pid"], ready.broker_pid);
        assert_eq!(endpoint["epoch"], ready.epoch);
    }

    fn enter(&self) {
        self.success(&self.output(&["enter", "personal"]), "enter personal");
    }

    fn exec(&self) -> Output {
        self.output(&["exec", "--no-resolve", "--", "/usr/bin/true"])
    }

    fn assert_execution_blocked(&self) {
        let marker = self.cwd.join("execution-must-not-start");
        let output = self.output(&[
            "exec",
            "--no-resolve",
            "--",
            "/usr/bin/touch",
            marker.to_str().unwrap(),
        ]);
        self.no_bearer(&output);
        assert!(
            !output.status.success(),
            "invalid authority permitted execution"
        );
        assert!(!marker.exists(), "child ran before authority refusal");
    }

    fn kill_owned_supervisor(&self, ready: &Ready) {
        // The PID came from our own successful launch handoff; confirm its
        // endpoint is still current before fault-injecting process death.
        self.verify_ready(ready);
        assert!(ready.supervisor_pid <= i32::MAX as u32);
        // SAFETY: this is the confirmed supervisor created by this fixture,
        // never a process discovered from the user's home or process table.
        assert_eq!(
            unsafe { libc::kill(ready.supervisor_pid as i32, libc::SIGKILL) },
            0
        );
        wait_until("dead supervisor fails closed", || {
            !self.output(&["whoami"]).status.success()
        });
        wait_until("dead supervisor retires broker", || {
            !self.endpoint_path().exists()
        });
    }
}

fn wait_until(description: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out: {description}");
        thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn pin_survives_entering_shell_exit_but_dead_supervisor_fails_closed() {
    let fixture = Fixture::new();
    let output = fixture
        .shell("\"$0\" --json supervisor start && \"$0\" enter personal >/dev/null; exit $?")
        .output()
        .unwrap();
    fixture.success(&output, "start and enter from short-lived shell");
    let ready: Ready = serde_json::from_slice(&output.stdout).unwrap();
    fixture.verify_ready(&ready);
    fixture.success(
        &fixture.output(&["whoami"]),
        "pin after entering shell exit",
    );
    fixture.success(&fixture.exec(), "execution after entering shell exit");

    fixture.kill_owned_supervisor(&ready);
    let whoami = fixture.output(&["whoami"]);
    fixture.no_bearer(&whoami);
    assert!(!whoami.status.success());
    assert!(String::from_utf8_lossy(&whoami.stderr).contains("human re-pin required"));
    fixture.assert_execution_blocked();
}

#[test]
fn supervisor_restart_does_not_revive_old_pin_generation() {
    let fixture = Fixture::new();
    let original = fixture.start();
    fixture.enter();
    let pin = fs::read(fixture.home.join("sessions/active.json")).unwrap();
    fixture.kill_owned_supervisor(&original);

    let replacement = fixture.start();
    assert_ne!(original.epoch, replacement.epoch);
    assert!(
        fs::read(fixture.home.join("sessions/active.json")).unwrap() == pin,
        "supervisor operation changed the sealed pin"
    );
    assert!(!fixture.output(&["whoami"]).status.success());
    fixture.assert_execution_blocked();
    fixture.enter();
    fixture.success(&fixture.exec(), "explicit human re-entry after restart");
}

#[test]
fn start_requires_explicit_capability_and_refuses_delegated_contexts() {
    let fixture = Fixture::new();
    let missing = fixture
        .command(&["supervisor", "start"])
        .env_remove("LOCUS_CONTROL_CAPABILITY")
        .output()
        .unwrap();
    assert!(
        !missing.status.success(),
        "persisted capability was implicitly adopted"
    );
    fixture.no_bearer(&missing);
    assert!(!fixture.endpoint_path().exists());

    for (name, value) in [
        ("LOCUS_EXECUTOR_CAPABILITY", "synthetic-delegation"),
        ("LOCUS_SESSION_ID", "synthetic-session"),
    ] {
        let output = fixture
            .command(&["supervisor", "start"])
            .env(name, value)
            .output()
            .unwrap();
        fixture.no_bearer(&output);
        assert!(
            !output.status.success(),
            "delegated context started supervisor"
        );
        assert!(!fixture.endpoint_path().exists());
    }
}

#[test]
fn wrong_capability_cannot_take_over_live_supervisor_or_change_pin() {
    let fixture = Fixture::new();
    let ready = fixture.start();
    fixture.enter();
    let pin = fs::read(fixture.home.join("sessions/active.json")).unwrap();
    let output = fixture
        .command(&["supervisor", "start"])
        .env("LOCUS_CONTROL_CAPABILITY", "b2".repeat(32))
        .output()
        .unwrap();
    fixture.no_bearer(&output);
    assert!(
        !output.status.success(),
        "wrong capability took over supervision"
    );
    fixture.verify_ready(&ready);
    assert!(
        fs::read(fixture.home.join("sessions/active.json")).unwrap() == pin,
        "supervisor operation changed the sealed pin"
    );
    fixture.success(
        &fixture.exec(),
        "original authority after rejected takeover",
    );
}

#[test]
fn existing_shell_broker_is_not_silently_claimed_as_durable() {
    let fixture = Fixture::new();
    fixture.enter();
    let before = fixture.endpoint();
    let pin = fs::read(fixture.home.join("sessions/active.json")).unwrap();
    let output = fixture.output(&["supervisor", "start"]);
    fixture.no_bearer(&output);
    assert!(
        !output.status.success(),
        "shell-owned authority was reported durable"
    );
    assert_eq!(fixture.endpoint(), before);
    assert!(
        fs::read(fixture.home.join("sessions/active.json")).unwrap() == pin,
        "supervisor operation changed the sealed pin"
    );
    fixture.success(&fixture.exec(), "shell-owned pin after refused adoption");
}

#[test]
fn concurrent_and_duplicate_starts_keep_exactly_one_broker_owner() {
    let fixture = Fixture::new();
    let mut first = fixture.command(&["--json", "supervisor", "start"]);
    let mut second = fixture.command(&["--json", "supervisor", "start"]);
    let first = first
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let second = second
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let outputs = [
        first.wait_with_output().unwrap(),
        second.wait_with_output().unwrap(),
    ];
    for output in &outputs {
        fixture.no_bearer(output);
    }
    assert_eq!(
        outputs
            .iter()
            .filter(|output| output.status.success())
            .count(),
        1
    );
    let winner = outputs
        .iter()
        .find(|output| output.status.success())
        .unwrap();
    let ready: Ready = serde_json::from_slice(&winner.stdout).unwrap();
    fixture.verify_ready(&ready);
    let duplicate = fixture.output(&["supervisor", "start"]);
    fixture.no_bearer(&duplicate);
    assert!(!duplicate.status.success());
    fixture.verify_ready(&ready);
    assert!(
        !fixture.home.join("sessions/active.json").exists(),
        "starting supervisor minted a pin"
    );
    fixture.enter();
    fixture.success(&fixture.exec(), "single winning supervisor");
}

struct OwnedChild(Child);

impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn foreground_supervisor_exits_when_its_home_is_removed() {
    let fixture = Fixture::new();
    let mut child = OwnedChild(
        fixture
            .command(&["supervisor", "run"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let stdout = child.0.stdout.take().unwrap();
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut line = String::new();
        let result = BufReader::new(stdout).read_line(&mut line).map(|_| line);
        let _ = sender.send(result);
    });
    let line = receiver
        .recv_timeout(Duration::from_secs(50))
        .unwrap()
        .unwrap();
    assert!(
        !line.contains(&fixture.capability),
        "foreground readiness disclosed capability"
    );
    assert!(line.contains("Supervisor ready"));
    let endpoint = fixture.endpoint();
    assert_eq!(endpoint["supervisor"]["pid"], child.0.id());
    assert_ne!(endpoint["owner_pid"], child.0.id());
    fixture.enter();
    fixture.success(&fixture.exec(), "foreground supervisor authority");
    fs::remove_dir_all(&fixture.home).unwrap();
    wait_until("foreground supervisor cleanup", || {
        child.0.try_wait().unwrap().is_some()
    });
}

#[test]
fn wrong_capability_cannot_start_an_idle_initialized_home() {
    let fixture = Fixture::new();
    let output = fixture
        .command(&["supervisor", "start"])
        .env("LOCUS_CONTROL_CAPABILITY", "b2".repeat(32))
        .output()
        .unwrap();
    fixture.no_bearer(&output);
    assert!(
        !output.status.success(),
        "wrong capability started idle home"
    );
    assert!(!fixture.endpoint_path().exists());
    assert!(!fixture.home.join("sessions/active.json").exists());
    fixture.start();
    fixture.enter();
    fixture.success(&fixture.exec(), "correct capability after refused start");
}

#[test]
fn dry_run_never_launches_a_supervisor() {
    let fixture = Fixture::new();
    for mode in ["start", "run"] {
        let output = fixture.output(&["--dry-run", "supervisor", mode]);
        fixture.no_bearer(&output);
        assert!(!output.status.success(), "unsupported dry-run must refuse");
        assert!(!fixture.endpoint_path().exists());
        assert!(!fixture.home.join("sessions/active.json").exists());
    }
}

#[test]
fn invalid_and_nonunicode_capability_errors_never_disclose_input() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let fixture = Fixture::new();
    let sentinel = "synthetic-private-sentinel";
    let mut invalid_unicode = sentinel.as_bytes().to_vec();
    invalid_unicode.push(0xff);
    for value in [
        OsString::from(sentinel),
        OsString::from_vec(invalid_unicode),
    ] {
        let output = fixture
            .command(&["supervisor", "start"])
            .env("LOCUS_CONTROL_CAPABILITY", value)
            .output()
            .unwrap();
        assert!(!output.status.success());
        for bytes in [&output.stdout, &output.stderr] {
            assert!(
                !String::from_utf8_lossy(bytes).contains(sentinel),
                "invalid capability input appeared in diagnostics"
            );
        }
        assert!(!fixture.endpoint_path().exists());
    }
}

#[test]
fn broken_readiness_stdout_retires_the_unreported_supervisor() {
    let fixture = Fixture::new();
    let mut child = fixture
        .command(&["--json", "supervisor", "start"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Close the parent's read end before the CLI publishes readiness. The
    // newly created daemon must not survive a failed operator handoff.
    drop(child.stdout.take());
    let output = child.wait_with_output().unwrap();
    fixture.no_bearer(&output);
    assert!(
        !output.status.success(),
        "broken readiness output reported success"
    );
    wait_until("unreported supervisor retirement", || {
        !fixture.endpoint_path().exists()
    });
    assert!(!fixture.home.join("sessions/active.json").exists());
    fixture.start();
    fixture.enter();
    fixture.success(
        &fixture.exec(),
        "fresh start after failed readiness handoff",
    );
}
