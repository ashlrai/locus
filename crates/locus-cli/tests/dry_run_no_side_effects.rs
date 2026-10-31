//! Preview fixtures have no broker and no pre-created migration lock.
//! Include runtime and metadata: byte-only snapshots miss mkdir/chmod/rewrites.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

#[derive(PartialEq)]
struct Entry {
    contents: Vec<u8>,
    modified: SystemTime,
    permissions: fs::Permissions,
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Entry> {
    fn visit(root: &Path, path: &Path, out: &mut BTreeMap<PathBuf, Entry>) {
        let meta = fs::symlink_metadata(path).unwrap();
        let contents = if meta.is_dir() {
            b"directory".to_vec()
        } else if meta.file_type().is_symlink() {
            fs::read_link(path)
                .unwrap()
                .as_os_str()
                .as_encoded_bytes()
                .to_vec()
        } else {
            assert!(meta.is_file(), "unexpected fixture type");
            fs::read(path).unwrap()
        };
        out.insert(
            path.strip_prefix(root).unwrap().into(),
            Entry {
                contents,
                modified: meta.modified().unwrap(),
                permissions: meta.permissions(),
            },
        );
        if meta.is_dir() {
            for child in fs::read_dir(path).unwrap() {
                visit(root, &child.unwrap().path(), out);
            }
        }
    }
    let mut out = BTreeMap::new();
    visit(root, root, &mut out);
    out
}

fn preview(root: &Path, args: &[&str], success: bool) {
    for json in [false, true] {
        let before = snapshot(root);
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_locus"));
        cmd.env_clear()
            .env("HOME", root.join("user"))
            .env("LOCUS_HOME", root.join("locus"))
            .env("NO_COLOR", "1")
            .env("LOCUS_NON_INTERACTIVE", "1")
            // A preview must not authenticate control or start its broker.
            .env("LOCUS_CONTROL_CAPABILITY", "invalid-preview-fixture")
            .current_dir(root.join("project"))
            .args(["--dry-run"])
            .args(args);
        // An instrumented child (cargo llvm-cov) writes a .profraw on exit.
        // env_clear drops LLVM_PROFILE_FILE, so the runtime would fall back
        // to default_*.profraw in the child's cwd — inside the snapshot.
        let profile = std::env::var_os("LLVM_PROFILE_FILE").map(PathBuf::from);
        let profile = profile
            .map(|p| std::path::absolute(&p).unwrap_or(p))
            .unwrap_or_else(|| std::env::temp_dir().join("locus-test-%p-%m.profraw"));
        cmd.env("LLVM_PROFILE_FILE", profile);
        if json {
            cmd.arg("--json");
        }
        let output = cmd.output().unwrap();
        let after = snapshot(root);
        // Never dump snapshot contents: they include a generated seal key.
        let changed = before
            .keys()
            .chain(after.keys())
            .find(|p| before.get(*p) != after.get(*p));
        assert!(
            changed.is_none(),
            "preview changed files or metadata at {changed:?}: {args:?}"
        );
        assert_eq!(output.status.success(), success, "preview status: {args:?}");
        if success && json {
            let plan: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(plan["schema"], "locus.plan.v1");
            assert_eq!(plan["dry_run"], true);
        }
    }
}

#[test]
fn previews_preserve_missing_and_initialized_homes() {
    for initialized in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir(root.join("user")).unwrap();
        fs::create_dir(root.join("project")).unwrap();
        if initialized {
            let store = locus_core::Store::open(root.join("locus")).unwrap();
            // Write directly so fixture setup cannot hide creation of a lock.
            fs::write(
                store.bindings_dir().join("acme.toml"),
                r#"
[binding]
id = "bnd_acme"
alias = "acme"
tenant = "acme"
[[binding.providers]]
provider = "github"
account = "acme"
credential_ref = "env:PREVIEW_FIXTURE"
"#,
            )
            .unwrap();
            fs::write(store.home().join("runtime/sentinel"), "runtime fixture").unwrap();
            fs::write(store.home().join("audit/events.jsonl"), "audit fixture\n").unwrap();
            fs::remove_dir(store.home().join("archives")).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(store.home(), fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        for command in [
            "init --with-samples",
            "quickstart",
            "setup --client cursor --mcp-bin /fixture/locus-mcp",
            "workspace --default acme",
            "doctor --fix",
            "binding add acme --tenant acme --provider github --account acme --credential-ref env:PREVIEW_FIXTURE --non-interactive",
        ] {
            preview(root, &command.split_whitespace().collect::<Vec<_>>(), true);
        }
        if initialized {
            for args in [
                &["pin", "acme"][..],
                &["binding", "rm", "acme", "--yes"],
                &["capability", "persist"],
            ] {
                preview(root, args, true);
            }
        }
        preview(root, &["binding", "rm", "missing", "--yes"], false);
        preview(root, &["pin", "missing"], false);
        if initialized {
            let lock = root.join("locus/bindings/.credential-migrations.lock");
            fs::write(&lock, "existing lock fixture").unwrap();
            preview(root, &["pin", "acme"], true);
            fs::remove_file(&lock).unwrap();
            fs::create_dir(&lock).unwrap();
            preview(root, &["pin", "acme"], false);
            #[cfg(unix)]
            {
                fs::remove_dir(&lock).unwrap();
                let target = root.join("locus/runtime/sentinel");
                std::os::unix::fs::symlink(&target, &lock).unwrap();
                preview(root, &["pin", "acme"], false);
                fs::remove_file(&lock).unwrap();
                fs::hard_link(target, &lock).unwrap();
                preview(root, &["pin", "acme"], false);
            }
        }
    }
}
