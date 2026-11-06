//! Issue #171: every reported mutation must support a side-effect-free preview.
//! Each case runs the binary in human and JSON modes, requiring exit 0 and
//! identical contents, paths, permissions and modification times throughout
//! the user home, Locus home and workspace (including directory metadata).

use std::fs;

use super::dry_run_no_side_effects::preview;
#[cfg(unix)]
use super::dry_run_no_side_effects::preview_with_path;

fn fixture() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir(root.join("user")).unwrap();
    fs::create_dir(root.join("project")).unwrap();
    let store = locus_core::Store::open(root.join("locus")).unwrap();
    fs::write(
        store.bindings_dir().join("acme.toml"),
        r#"
[binding]
id = "bnd_issue171"
alias = "acme"
tenant = "issue171"
[[binding.providers]]
provider = "github"
account = "issue171"
credential_ref = "env:ISSUE171_UNUSED"
"#,
    )
    .unwrap();
    // No broker, capability, credentials or migration lock is prepared.
    // Planning must not create any of them, even for enter/pin/quickstart.
    fs::write(store.home().join("runtime/sentinel"), "preserve runtime").unwrap();
    fs::write(
        root.join("user/.profile"),
        "# preserve user configuration\n",
    )
    .unwrap();
    temp
}

macro_rules! matrix_case {
    ($name:ident, [$($arg:expr),+ $(,)?]) => {
        #[test]
        fn $name() {
            let temp = fixture();
            preview(temp.path(), &[$($arg),+], true);
        }
    };
}

matrix_case!(init, ["init", "--with-samples"]);
matrix_case!(quickstart, ["quickstart"]);
matrix_case!(enter, ["enter", "acme"]);
matrix_case!(pin, ["pin", "acme"]);
matrix_case!(binding_rm, ["binding", "rm", "acme", "--yes"]);
matrix_case!(workspace, ["workspace", "--default", "acme"]);
matrix_case!(
    engagement_init,
    [
        "engagement",
        "init",
        "new-client",
        "--tenant",
        "new-tenant",
        "--workspace"
    ]
);
matrix_case!(
    graph_export,
    [
        "graph",
        "export",
        "--bindings",
        "acme",
        "--out",
        "preview.locusgraph"
    ]
);
matrix_case!(onboard, ["onboard"]);

#[test]
fn leave() {
    let temp = fixture();
    preview(temp.path(), &["leave"], true);
    // Exercise an actual planned deletion as well as the unpinned no-op.
    fs::write(
        temp.path().join("locus/sessions/active.json"),
        "wedged fixture",
    )
    .unwrap();
    preview(temp.path(), &["leave", "--force"], true);
}

#[test]
fn adapter_install() {
    let temp = fixture();
    let home = temp.path().join("locus");
    locus_core::marketplace::save_index_sources(
        &home,
        &[locus_core::marketplace::IndexSource {
            name: "issue171".into(),
            url: "https://issue171.invalid/index.json".into(),
        }],
    )
    .unwrap();
    // RFC 8032 public test vector: public verification material only.
    locus_core::add_ed25519_trust_key(
        &home,
        "issue171",
        "11qYAYKxCrfVS/7TyWQHOg7hcvPapiMlrwIaaPcHURo=",
    )
    .unwrap();
    // The reserved .invalid index cannot provide a real manifest. A preview
    // must succeed locally without fetching or executing publisher code.
    preview(temp.path(), &["adapter", "install", "issue171"], true);
}

#[cfg(unix)]
#[test]
fn onboard_preview_never_runs_gh() {
    use std::os::unix::fs::PermissionsExt;
    let temp = fixture();
    let bin = tempfile::tempdir().unwrap();
    let gh = bin.path().join("gh");
    std::fs::write(&gh, "#!/bin/sh\nprintf called > \"$HOME/gh-was-called\"\n").unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o700)).unwrap();
    // Positive control: the fake probe really writes under the fixture HOME.
    let marker = temp.path().join("user/gh-was-called");
    let status = std::process::Command::new(&gh)
        .env("HOME", temp.path().join("user"))
        .status()
        .unwrap();
    assert!(status.success());
    assert!(marker.exists());
    std::fs::remove_file(&marker).unwrap();
    // Both human and JSON previews retain the full tree/metadata assertion.
    preview_with_path(temp.path(), &["onboard"], true, Some(bin.path()));
    assert!(!marker.exists());
}
