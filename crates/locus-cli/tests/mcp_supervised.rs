//! Contract for `locus mcp --supervised`.
//!
//! A directly launched locus-mcp runs executor-restricted with no executor
//! capability, so it serves control tools only. The supervised launch grants
//! the active pin's executor capability to the child, with the same isolated
//! env as `locus exec --no-resolve`, and falls back to an unsupervised launch
//! (control tools only) when it cannot grant one.
//!
//! The child here is a stub that records its environment, so the test pins
//! what locus hands to locus-mcp without depending on the locus-mcp binary.
//! The authority broker is supervised by this test process (the parent of
//! `locus enter`), so it stays alive for the whole test.
#![cfg(unix)]

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output, Stdio};

const CONTROL: &str = "0b5e7a1c9d2f4e6a8b0c1d3e5f7a9b1c2d4e6f8a0b1c3d5e7f9a1b2c4d6e8f0a";

fn locus(home: &Path, cwd: &Path, control: bool, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_locus"));
    cmd.args(args)
        .env("LOCUS_HOME", home)
        .env_remove("LOCUS_CONTROL_CAPABILITY")
        .env_remove("LOCUS_EXECUTOR_CAPABILITY")
        .env_remove("LOCUS_SESSION_ID")
        .env("LOCUS_CLIENT", "claude")
        .current_dir(cwd)
        .stdin(Stdio::null());
    if control {
        cmd.env("LOCUS_CONTROL_CAPABILITY", CONTROL);
    }
    cmd.output().expect("locus binary runs")
}

/// Stub locus-mcp: dump its env (NUL-separated) to `out` and exit 0.
fn env_recorder(dir: &Path, out: &Path) -> String {
    let script = dir.join("fake-locus-mcp");
    std::fs::write(
        &script,
        format!("#!/bin/sh\n/usr/bin/env -0 > '{}'\n", out.display()),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    script.display().to_string()
}

fn recorded_env(path: &Path) -> BTreeMap<String, String> {
    let raw = std::fs::read(path).expect("stub locus-mcp ran");
    raw.split(|b| *b == 0)
        .filter(|kv| !kv.is_empty())
        .filter_map(|kv| {
            let kv = String::from_utf8_lossy(kv);
            kv.split_once('=')
                .map(|(k, v)| (k.to_string(), v.to_string()))
        })
        .collect()
}

fn init_store(home: &Path, cwd: &Path) {
    let init = locus(
        home,
        cwd,
        true,
        &["init", "--with-samples", "--no-persist-capability"],
    );
    assert!(init.status.success(), "init failed: {init:?}");
}

#[test]
fn supervised_launch_hands_child_executor_capability_and_no_control_authority() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    init_store(home.path(), cwd.path());
    let enter = locus(home.path(), cwd.path(), true, &["enter", "personal"]);
    assert!(enter.status.success(), "enter failed: {enter:?}");

    let out = cwd.path().join("env.bin");
    let stub = env_recorder(cwd.path(), &out);
    let run = locus(
        home.path(),
        cwd.path(),
        true,
        &["mcp", "--supervised", "--mcp-bin", &stub],
    );
    assert!(run.status.success(), "supervised launch failed: {run:?}");
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains("supervised as personal"),
        "stderr: {stderr}"
    );
    assert!(run.stdout.is_empty(), "stdout is the MCP channel: {run:?}");

    let env = recorded_env(&out);
    let cap = env
        .get("LOCUS_EXECUTOR_CAPABILITY")
        .expect("child gets the executor capability");
    assert_eq!(cap.len(), 64, "executor capability is 32 bytes hex");
    assert!(
        !env.contains_key("LOCUS_CONTROL_CAPABILITY"),
        "control authority must never reach the agent-facing server"
    );
    assert_eq!(
        std::fs::canonicalize(env.get("LOCUS_HOME").expect("LOCUS_HOME is set")).unwrap(),
        std::fs::canonicalize(home.path()).unwrap(),
        "LOCUS_HOME stays scoped to the configured Locus home"
    );
    let worker_home = env.get("LOCUS_WORKER_HOME").expect("worker home is set");
    let child_home = env.get("HOME").expect("child HOME is set");
    assert_eq!(
        std::fs::canonicalize(worker_home).unwrap(),
        std::fs::canonicalize(child_home).unwrap(),
        "HOME is the isolated worker home, as under locus exec"
    );
    assert_eq!(env.get("LOCUS_CLIENT").map(String::as_str), Some("claude"));

    let audit = std::fs::read_dir(home.path().join("audit"))
        .unwrap()
        .filter_map(|e| std::fs::read_to_string(e.unwrap().path()).ok())
        .collect::<String>();
    assert!(
        audit.contains("session.mcp.supervised"),
        "supervised launch is audited: {audit}"
    );
}

#[test]
fn supervised_launch_without_pin_falls_back_to_control_tools_only() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    init_store(home.path(), cwd.path());

    let out = cwd.path().join("env.bin");
    let stub = env_recorder(cwd.path(), &out);
    let run = locus(
        home.path(),
        cwd.path(),
        true,
        &["mcp", "--supervised", "--mcp-bin", &stub],
    );
    assert!(run.status.success(), "fallback still serves: {run:?}");
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains("serving control tools only") && stderr.contains("active pin"),
        "stderr: {stderr}"
    );
    assert!(!recorded_env(&out).contains_key("LOCUS_EXECUTOR_CAPABILITY"));
}

#[test]
fn supervised_launch_without_control_capability_grants_nothing() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    init_store(home.path(), cwd.path());
    let enter = locus(home.path(), cwd.path(), true, &["enter", "personal"]);
    assert!(enter.status.success(), "enter failed: {enter:?}");

    let out = cwd.path().join("env.bin");
    let stub = env_recorder(cwd.path(), &out);
    let run = locus(
        home.path(),
        cwd.path(),
        false,
        &["mcp", "--supervised", "--mcp-bin", &stub],
    );
    assert!(run.status.success(), "fallback still serves: {run:?}");
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains("serving control tools only")
            && stderr.contains("LOCUS_CONTROL_CAPABILITY"),
        "stderr: {stderr}"
    );
    assert!(!recorded_env(&out).contains_key("LOCUS_EXECUTOR_CAPABILITY"));
}

#[test]
fn supervised_conflicts_with_grant_subcommands() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let run = locus(
        home.path(),
        cwd.path(),
        true,
        &["mcp", "--supervised", "list"],
    );
    assert!(!run.status.success());
}

#[test]
fn fallback_strips_an_inherited_executor_capability() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    init_store(home.path(), cwd.path());

    let out = cwd.path().join("env.bin");
    let stub = env_recorder(cwd.path(), &out);
    let run = Command::new(env!("CARGO_BIN_EXE_locus"))
        .args(["mcp", "--supervised", "--mcp-bin", &stub])
        .env("LOCUS_HOME", home.path())
        .env("LOCUS_CONTROL_CAPABILITY", CONTROL)
        .env("LOCUS_EXECUTOR_CAPABILITY", "f".repeat(64))
        .env_remove("LOCUS_SESSION_ID")
        .current_dir(cwd.path())
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(run.status.success(), "fallback still serves: {run:?}");
    assert!(
        !recorded_env(&out).contains_key("LOCUS_EXECUTOR_CAPABILITY"),
        "an unsupervised fallback never forwards a stray executor capability"
    );
}

#[test]
fn supervised_under_locus_exec_passes_the_parent_grant_through() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    init_store(home.path(), cwd.path());
    let enter = locus(home.path(), cwd.path(), true, &["enter", "personal"]);
    assert!(enter.status.success(), "enter failed: {enter:?}");

    let out = cwd.path().join("env.bin");
    let stub = env_recorder(cwd.path(), &out);
    let locus_bin = env!("CARGO_BIN_EXE_locus");
    let run = locus(
        home.path(),
        cwd.path(),
        true,
        &[
            "exec",
            "--no-resolve",
            "--",
            locus_bin,
            "mcp",
            "--supervised",
            "--mcp-bin",
            &stub,
        ],
    );
    assert!(run.status.success(), "nested launch failed: {run:?}");
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(stderr.contains("already supervised"), "stderr: {stderr}");
    let env = recorded_env(&out);
    assert_eq!(
        env.get("LOCUS_EXECUTOR_CAPABILITY").map(String::len),
        Some(64),
        "the parent's grant reaches the server"
    );
    assert!(!env.contains_key("LOCUS_CONTROL_CAPABILITY"));
}
