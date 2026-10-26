//! Process-level regressions: a stdio session belongs to its client, and an
//! upstream worker must never turn back into another Locus multiplexor.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn server() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_locus-mcp"));
    command.env_clear().env("LOCUS_MCP_AUTO_PIN", "0");
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn wait(child: &mut Child) -> std::process::ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("locus-mcp did not exit when its session ended");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn upstream_child_marker_refuses_recursive_multiplexor_before_reading_stdin() {
    let home = tempfile::tempdir().unwrap();
    let mut child = server()
        .env("HOME", home.path())
        .env("LOCUS_HOME", home.path())
        .env("LOCUS_MCP_UPSTREAM_CHILD", "1")
        .spawn()
        .unwrap();
    // Keep stdin open: failing closed must not depend on a client disconnect.
    assert!(!wait(&mut child).success());
    let mut stdout = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    assert!(stdout.is_empty(), "diagnostics must not corrupt MCP stdout");
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(stderr.contains("recursive"), "{stderr}");
}

#[test]
fn twenty_stdio_sessions_exit_after_client_eof() {
    let home = tempfile::tempdir().unwrap();
    for _ in 0..20 {
        let mut child = server()
            .env("HOME", home.path())
            .env("LOCUS_HOME", home.path())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        writeln!(stdin, r#"{{"jsonrpc":"2.0","id":1,"method":"ping"}}"#).unwrap();
        drop(stdin);
        assert!(wait(&mut child).success());
        let mut output = String::new();
        child
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut output)
            .unwrap();
        let reply: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
        assert_eq!(reply["id"], 1);
        assert_eq!(reply["result"], serde_json::json!({}));
    }
}

#[cfg(unix)]
#[test]
fn parent_death_exits_even_when_another_process_holds_partial_stdin_open() {
    let home = tempfile::tempdir().unwrap();
    let mut parent = Command::new("/bin/sh")
        .args([
            "-c",
            "\"$1\" <&0 & child=$!; echo $child; wait $child",
            "parent",
        ])
        .arg(env!("CARGO_BIN_EXE_locus-mcp"))
        .env_clear()
        .env("HOME", home.path())
        .env("LOCUS_HOME", home.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = parent.stdin.take().unwrap();
    let mut stdout = BufReader::new(parent.stdout.take().unwrap());
    let mut pid = String::new();
    stdout.read_line(&mut pid).unwrap();
    let pid: i32 = pid.trim().parse().unwrap();
    writeln!(stdin, r#"{{"jsonrpc":"2.0","id":1,"method":"ping"}}"#).unwrap();
    let mut response = String::new();
    stdout.read_line(&mut response).unwrap();
    assert!(
        response.contains("result"),
        "server must be running before parent dies"
    );
    stdin.write_all(b"Content-Length: 100\r\n\r\n{").unwrap();
    stdin.flush().unwrap();
    parent.kill().unwrap();
    parent.wait().unwrap();
    let (send, recv) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut rest = String::new();
        let _ = send.send(stdout.read_to_string(&mut rest));
    });
    let result = recv.recv_timeout(Duration::from_secs(5));
    // Always clean the fixture up, including on the unfixed implementation.
    if result.is_err() {
        let _ = Command::new("/bin/kill")
            .args(["-KILL", &pid.to_string()])
            .status();
    }
    drop(stdin);
    reader.join().unwrap();
    assert!(
        result.is_ok(),
        "orphaned MCP kept waiting on inherited stdin"
    );
}

#[cfg(unix)]
fn fill_response_pipe(stdin: &mut impl Write) {
    // Echoed ids are valid JSON-RPC. This response exceeds pipe capacity, so
    // dispatch blocks in stdout until the lifetime monitor terminates it.
    let request = serde_json::json!({
        "jsonrpc": "2.0", "id": "x".repeat(2 * 1024 * 1024), "method": "ping"
    });
    writeln!(stdin, "{request}").unwrap();
    stdin.flush().unwrap();
}

#[cfg(unix)]
#[test]
fn stdin_eof_exits_even_when_client_stops_reading_stdout() {
    let home = tempfile::tempdir().unwrap();
    let mut child = server()
        .env("HOME", home.path())
        .env("LOCUS_HOME", home.path())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    fill_response_pipe(&mut stdin);
    drop(stdin);
    // Deliberately retain the unread output pipe. wait() cleans up the child
    // before failing if the implementation cannot escape its blocked write.
    assert!(wait(&mut child).success());
}

#[cfg(unix)]
#[test]
fn parent_death_exits_even_when_client_stops_reading_stdout() {
    use std::os::fd::AsRawFd;

    let home = tempfile::tempdir().unwrap();
    let mut parent = Command::new("/bin/sh")
        .args([
            "-c",
            "\"$1\" <&0 & child=$!; echo $child; wait $child",
            "parent",
        ])
        .arg(env!("CARGO_BIN_EXE_locus-mcp"))
        .env_clear()
        .env("HOME", home.path())
        .env("LOCUS_HOME", home.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = parent.stdin.take().unwrap();
    let mut stdout = BufReader::new(parent.stdout.take().unwrap());
    let mut pid = String::new();
    stdout.read_line(&mut pid).unwrap();
    let pid: i32 = pid.trim().parse().unwrap();
    fill_response_pipe(&mut stdin);
    parent.kill().unwrap();
    parent.wait().unwrap();

    // POLLHUP observes writer closure without draining the response (which
    // would unblock dispatch and hide the bug). It also works if the orphan
    // is temporarily a zombie waiting for the OS's reaper.
    let mut descriptor = libc::pollfd {
        fd: stdout.get_ref().as_raw_fd(),
        events: libc::POLLHUP,
        revents: 0,
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    let closed = loop {
        // SAFETY: descriptor points to one valid pollfd, and stdout owns fd.
        unsafe { libc::poll(&mut descriptor, 1, 100) };
        if descriptor.revents & libc::POLLHUP != 0 {
            break true;
        }
        if Instant::now() >= deadline {
            break false;
        }
    };
    if !closed {
        let _ = Command::new("/bin/kill")
            .args(["-KILL", &pid.to_string()])
            .status();
    }
    drop(stdin);
    assert!(
        closed,
        "orphaned MCP remained blocked writing client stdout"
    );
}
