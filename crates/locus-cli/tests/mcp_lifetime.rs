//! Exercise the extra `locus mcp` supervisor, not just the native MCP server.
#![cfg(unix)]

use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Fixture {
    parent: Child,
    cli_pid: PathBuf,
    worker_pid: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.parent.kill();
        let _ = self.parent.wait();
        for path in [&self.worker_pid, &self.cli_pid] {
            if let Ok(pid) = std::fs::read_to_string(path) {
                if let Ok(pid) = pid.trim().parse::<i32>() {
                    if pid > 1 {
                        // SAFETY: these are fixture-owned processes; the worker
                        // is a group leader only on the fixed implementation.
                        unsafe {
                            libc::kill(-pid, libc::SIGKILL);
                            libc::kill(pid, libc::SIGKILL);
                        }
                    }
                }
            }
        }
    }
}

fn wait_for(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !std::fs::read_to_string(path).is_ok_and(|value| value.trim().parse::<u32>().is_ok()) {
        assert!(Instant::now() < deadline, "MCP fixture did not start");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn supervised_cli_exits_with_client_even_when_another_process_holds_stdin() {
    let home = tempfile::tempdir().unwrap();
    let worker_pid = home.path().join("worker.pid");
    let cli_pid = home.path().join("cli.pid");
    let binary = home.path().join("fake-locus-mcp");
    std::fs::write(
        &binary,
        format!(
            "#!/bin/sh\ntrap '' TERM\necho $$ > '{}'\nexec /bin/sleep 60\n",
            worker_pid.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    let parent = Command::new("/bin/sh")
        .args([
            "-c",
            "\"$1\" mcp --supervised --mcp-bin \"$2\" <&0 & child=$!; echo $child > \"$3\"; wait $child",
            "client",
        ])
        .arg(env!("CARGO_BIN_EXE_locus"))
        .arg(&binary)
        .arg(&cli_pid)
        .env_clear()
        .env("HOME", home.path())
        .env("LOCUS_HOME", home.path().join("locus"))
        .env("PATH", "/usr/bin:/bin")
        .current_dir(home.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut fixture = Fixture {
        parent,
        cli_pid,
        worker_pid,
    };
    wait_for(&fixture.worker_pid);
    wait_for(&fixture.cli_pid);
    // Retain the write end so neither CLI nor native child can observe EOF.
    let _input = fixture.parent.stdin.take().unwrap();
    let output = fixture.parent.stdout.take().unwrap();
    fixture.parent.kill().unwrap();
    fixture.parent.wait().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut descriptor = libc::pollfd {
        fd: output.as_raw_fd(),
        events: libc::POLLHUP,
        revents: 0,
    };
    loop {
        // SAFETY: output owns the descriptor and poll receives one valid entry.
        unsafe { libc::poll(&mut descriptor, 1, 100) };
        if descriptor.revents & libc::POLLHUP != 0 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "orphaned locus CLI/MCP retained client stdout"
        );
    }
}

#[test]
fn supervised_cli_preserves_exit_code_and_collects_remaining_descendants() {
    let home = tempfile::tempdir().unwrap();
    let binary = home.path().join("fake-locus-mcp");
    let worker_pid = home.path().join("descendant.pid");
    std::fs::write(
        &binary,
        format!(
            "#!/bin/sh\n/bin/sleep 60 &\necho $! > '{}'\nexit 7\n",
            worker_pid.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    let parent = Command::new(env!("CARGO_BIN_EXE_locus"))
        .args(["mcp", "--supervised", "--mcp-bin"])
        .arg(binary)
        .env_clear()
        .env("HOME", home.path())
        .env("LOCUS_HOME", home.path().join("locus"))
        .current_dir(home.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut fixture = Fixture {
        parent,
        cli_pid: home.path().join("absent"),
        worker_pid,
    };
    let output = fixture.parent.stdout.take().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = fixture.parent.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "supervised CLI did not exit");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(status.code(), Some(7));
    let mut descriptor = libc::pollfd {
        fd: output.as_raw_fd(),
        events: libc::POLLHUP,
        revents: 0,
    };
    loop {
        // SAFETY: output owns the descriptor and poll receives one valid entry.
        unsafe { libc::poll(&mut descriptor, 1, 100) };
        if descriptor.revents & libc::POLLHUP != 0 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "MCP descendant retained stdout after child exit"
        );
    }
}
