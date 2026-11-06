//! E2E for #173: `locus whoami --json` `credential.present` reflects whether
//! the provider's env credential variable is set — probed without reading or
//! printing its value. Unset ⇒ `present: false`, and the credential object
//! carries no value field.

use std::process::{Command, Output};

/// env: refs on the `personal` sample binding written by `locus quickstart`.
const SUPABASE_VAR: &str = "LOCUS_SUPABASE_PERSONAL";
const GITHUB_VAR: &str = "LOCUS_GH_TOKEN_PERSONAL";
const VERCEL_VAR: &str = "LOCUS_VERCEL_TOKEN_PERSONAL";
/// Synthetic probe value; never a real credential.
const PROBE_VALUE: &str = "whoami-credential-presence-probe";

fn locus(
    home: &std::path::Path,
    cwd: &std::path::Path,
    cap: Option<&str>,
    args: &[&str],
) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_locus"));
    cmd.args(args)
        .env("LOCUS_HOME", home)
        .env("NO_COLOR", "1")
        .env_remove("LOCUS_CONTROL_CAPABILITY")
        .env_remove(SUPABASE_VAR)
        .env_remove(GITHUB_VAR)
        .env_remove(VERCEL_VAR)
        .current_dir(cwd);
    if let Some(cap) = cap {
        cmd.env("LOCUS_CONTROL_CAPABILITY", cap);
    }
    cmd.output().expect("locus binary runs")
}

/// quickstart + `enter personal`, returning (home, cwd, control capability).
fn pinned() -> (tempfile::TempDir, tempfile::TempDir, String) {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let qs = locus(home.path(), cwd.path(), None, &["quickstart"]);
    assert!(qs.status.success(), "quickstart failed: {qs:?}");
    let cap = std::fs::read_to_string(home.path().join("control_capability"))
        .expect("quickstart persists the control capability")
        .trim()
        .to_string();
    let enter = locus(home.path(), cwd.path(), Some(&cap), &["enter", "personal"]);
    assert!(
        enter.status.success(),
        "enter personal failed: {}",
        String::from_utf8_lossy(&enter.stderr)
    );
    (home, cwd, cap)
}

/// `locus whoami --json` with every credential env var scrubbed except the
/// explicit `set` overrides; returns the raw stdout and the parsed envelope.
fn whoami_json(
    home: &std::path::Path,
    cwd: &std::path::Path,
    cap: &str,
    set: &[(&str, &str)],
) -> (String, serde_json::Value) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_locus"));
    cmd.args(["whoami", "--json"])
        .env("LOCUS_HOME", home)
        .env("LOCUS_CONTROL_CAPABILITY", cap)
        .env("NO_COLOR", "1")
        .env_remove(SUPABASE_VAR)
        .env_remove(GITHUB_VAR)
        .env_remove(VERCEL_VAR)
        .current_dir(cwd);
    for (var, value) in set {
        cmd.env(var, value);
    }
    let out = cmd.output().expect("locus binary runs");
    assert!(
        out.status.success(),
        "whoami failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    let json = serde_json::from_str(&stdout).expect("whoami --json emits JSON");
    (stdout, json)
}

fn providers(json: &serde_json::Value) -> &[serde_json::Value] {
    json["providers"].as_array().expect("providers array")
}

fn credential<'a>(json: &'a serde_json::Value, provider: &str) -> &'a serde_json::Value {
    &providers(json)
        .iter()
        .find(|p| p["provider"] == provider)
        .unwrap_or_else(|| panic!("missing provider {provider} in whoami output"))["credential"]
}

/// The metadata contract: presence + source only — never a ref name or value.
fn assert_credential_shape(credential: &serde_json::Value) {
    let mut keys: Vec<&str> = credential
        .as_object()
        .expect("credential object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, ["present", "source"], "credential keys: {credential}");
}

#[test]
fn unset_env_reports_present_false_and_no_value_field() {
    let (home, cwd, cap) = pinned();
    let (stdout, json) = whoami_json(home.path(), cwd.path(), &cap, &[]);
    assert_eq!(json["schema"], "locus.whoami.v1");
    for p in providers(&json) {
        assert_eq!(p["credential"]["present"], false, "{p}");
        assert_credential_shape(&p["credential"]);
    }
    // The env locator name must not leak into agent-facing output either.
    for var in [SUPABASE_VAR, GITHUB_VAR, VERCEL_VAR] {
        assert!(!stdout.contains(var), "whoami leaks {var}: {stdout}");
    }
}

#[test]
fn set_env_reports_present_true_without_echoing_the_value() {
    let (home, cwd, cap) = pinned();
    let (stdout, json) = whoami_json(
        home.path(),
        cwd.path(),
        &cap,
        &[(SUPABASE_VAR, PROBE_VALUE)],
    );
    let supabase = credential(&json, "supabase");
    assert_eq!(supabase["present"], true, "{supabase}");
    assert_eq!(supabase["source"], "environment", "{supabase}");
    for provider in ["github", "vercel"] {
        assert_eq!(credential(&json, provider)["present"], false, "{provider}");
    }
    for p in providers(&json) {
        assert_credential_shape(&p["credential"]);
    }
    assert!(
        !stdout.contains(PROBE_VALUE),
        "whoami echoes the credential value: {stdout}"
    );

    // Presence follows the current process environment, including an empty
    // value (which the resolver accepts), rather than caching an earlier probe.
    let (_, empty) = whoami_json(home.path(), cwd.path(), &cap, &[(SUPABASE_VAR, "")]);
    assert_eq!(credential(&empty, "supabase")["present"], true);
    let (_, unset) = whoami_json(home.path(), cwd.path(), &cap, &[]);
    assert_eq!(credential(&unset, "supabase")["present"], false);
}
