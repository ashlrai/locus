#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

struct ServerGuard(std::process::Child);

impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

// The helper owns the only write end of the server's stdin. Killing it models
// a client crash while a native server is actively serving requests.
#[test]
fn native_server_exits_after_client_is_killed_ten_times() {
    if let Some(marker) = std::env::var_os("LOCUS_LIFECYCLE_MARKER") {
        let mut server = ServerGuard(
            Command::new(env!("CARGO_BIN_EXE_locus-mcp"))
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
        let mut client = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "native_server_exits_after_client_is_killed_ten_times",
                "--nocapture",
            ])
            .env("LOCUS_LIFECYCLE_MARKER", &marker)
            .env("HOME", home.path())
            .env("LOCUS_HOME", home.path().join("store"))
            .env_remove("LOCUS_EXECUTOR_CAPABILITY")
            .env_remove("LOCUS_CONTROL_CAPABILITY")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !marker.exists() && Instant::now() < deadline {
            assert!(
                client.try_wait().unwrap().is_none(),
                "client exited before server was ready"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        if !marker.exists() {
            let _ = client.kill();
            let _ = client.wait();
            panic!("server did not start");
        }
        let pid = std::fs::read_to_string(&marker).unwrap();
        client.kill().unwrap();
        client.wait().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let pid: libc::pid_t = pid.parse().unwrap();
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
            let result = unsafe { libc::kill(pid, 0) };
            let gone =
                result != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
            if gone || zombie {
                break;
            }
            if Instant::now() >= deadline {
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                }
                panic!("server survived client death in iteration {iteration}");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
