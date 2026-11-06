//! Missing agent-contract coverage recut from #358; #343 is already covered
//! by `whoami_reports_a_set_binding_credential_without_exposing_it` on main.

use serde_json::Value;
use std::process::Command;

#[test]
fn whoami_reports_empty_environment_credential_as_present() {
    const VAR: &str = "LOCUS_GH_TOKEN_PERSONAL";
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("locus");
    let user = root.path().join("user");
    let project = root.path().join("project");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::create_dir_all(&project).unwrap();
    let command = || {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_locus"));
        cmd.env_clear()
            .env("HOME", &user)
            .env("LOCUS_HOME", &home)
            .current_dir(&project);
        cmd
    };
    let setup = command().arg("quickstart").output().unwrap();
    assert!(setup.status.success(), "quickstart failed");
    let cap = std::fs::read_to_string(home.join("control_capability")).unwrap();

    // Availability means the reference resolves, not that its value is valid.
    // Keep the empty value scoped to this child; never inherit operator identity.
    let out = command()
        .args(["whoami", "--json"])
        .env("LOCUS_CONTROL_CAPABILITY", cap.trim())
        .env(VAR, "")
        .output()
        .unwrap();
    assert!(out.status.success(), "whoami failed");
    for stream in [&out.stdout, &out.stderr] {
        assert!(!stream.windows(VAR.len()).any(|w| w == VAR.as_bytes()));
        assert!(!stream
            .windows(cap.trim().len())
            .any(|w| w == cap.trim().as_bytes()));
    }
    let result: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(result["schema"], "locus.whoami.v1");
    assert_eq!(result["binding_alias"], "personal");
    let provider = result["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["provider"] == "github")
        .unwrap();
    assert_eq!(provider["credential"]["source"], "environment");
    assert_eq!(provider["credential"]["present"], true);
}
