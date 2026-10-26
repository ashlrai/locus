//! Provider CLI routes use synthetic credentials and fake executables only.
//! The probes check credentials in-process; no credential is printed or recorded.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

const CONTROL: &str = "0b5e7a1c9d2f4e6a8b0c1d3e5f7a9b1c2d4e6f8a0b1c3d5e7f9a1b2c4d6e8f0a";
const AWS: &str = r#"{"AWS_ACCESS_KEY_ID":"route-key-id","AWS_SECRET_ACCESS_KEY":"route-aws-secret","AWS_SESSION_TOKEN":"route-session-token"}"#;

struct Fixture {
    root: tempfile::TempDir,
}

impl Fixture {
    fn new(providers: &[(&str, &str)]) -> Self {
        let f = Self {
            root: tempfile::tempdir().unwrap(),
        };
        for directory in ["bin", "project", "user"] {
            std::fs::create_dir_all(f.path(directory)).unwrap();
        }
        let init = f.run(&["init", "--no-persist-capability"]);
        assert_ok(&init);
        let mut binding =
            String::from("id = \"bnd_route\"\nalias = \"route\"\ntenant = \"synthetic\"\n");
        for (provider, reference) in providers {
            binding.push_str(&format!(
                "\n[[providers]]\nprovider = {provider:?}\naccount = \"synthetic\"\ncredential_ref = {reference:?}\n"
            ));
        }
        std::fs::write(f.binding_path(), binding).unwrap();
        f
    }

    fn all() -> Self {
        Self::new(&[
            ("github", "env:ROUTE_GITHUB"),
            ("vercel", "env:ROUTE_VERCEL"),
            ("supabase", "env:ROUTE_SUPABASE"),
            ("aws", "env:ROUTE_AWS"),
        ])
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }

    fn binding_path(&self) -> PathBuf {
        self.path("locus/bindings/route.toml")
    }

    fn run(&self, args: &[&str]) -> Output {
        self.run_env(args, &[])
    }

    fn run_env(&self, args: &[&str], extra: &[(&str, &str)]) -> Output {
        let out = Command::new(env!("CARGO_BIN_EXE_locus"))
            .args(args)
            .env_clear()
            .env("HOME", self.path("user"))
            .env("USERPROFILE", self.path("user"))
            .env("LOCUS_HOME", self.path("locus"))
            .env("LOCUS_CONTROL_CAPABILITY", CONTROL)
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.path("bin").display()),
            )
            .env("NO_COLOR", "1")
            .env("ROUTE_GITHUB", "route-github-secret")
            .env("ROUTE_VERCEL", "route-vercel-secret")
            .env("ROUTE_SUPABASE", "route-supabase-secret")
            .env("ROUTE_AWS", AWS)
            .env("GH_TOKEN", "ambient-github")
            .env("VERCEL_TOKEN", "ambient-vercel")
            .env("SUPABASE_ACCESS_TOKEN", "ambient-supabase")
            .env("AWS_SECRET_ACCESS_KEY", "ambient-aws")
            .env("AWS_PROFILE", "ambient-profile")
            .env("AWS_ACCESS_KEY_ID", "ambient-key-id")
            .envs(extra.iter().copied())
            .current_dir(self.path("project"))
            .output()
            .expect("locus runs");
        // Assert this before any diagnostic output so a regression never prints a canary.
        for secret in [
            CONTROL,
            "route-github-secret",
            "route-vercel-secret",
            "route-supabase-secret",
            "route-aws-secret",
            "route-session-token",
            "ambient-github",
            "ambient-vercel",
            "ambient-supabase",
            "ambient-aws",
            "foreign-locale-canary",
            "foreign-language-canary",
        ] {
            assert!(
                !String::from_utf8_lossy(&out.stdout).contains(secret)
                    && !String::from_utf8_lossy(&out.stderr).contains(secret),
                "CLI output disclosed a synthetic credential"
            );
        }
        out
    }

    fn pin(&self) {
        assert_ok(&self.run(&["enter", "route"]));
    }

    fn install(&self, cli: &str, body: &str) -> PathBuf {
        let path = self.path(&format!("bin/{cli}"));
        std::fs::write(
            &path,
            format!("#!/bin/sh\nset -eu\n: > child-started\n{body}\n"),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn refused(&self, args: &[&str]) {
        let out = self.run(args);
        assert!(!out.status.success(), "route unexpectedly succeeded");
        assert!(
            !self.path("project/child-started").exists(),
            "refused child started"
        );
    }

    fn route_probe(&self, cli: &str, provider: &str) {
        let mut script = format!("[ \"$LOCUS_PROVIDERS\" = {provider} ]\n");
        script.push_str(
            r#"
[ -z "${LOCUS_HOME+x}" ]
[ -z "${LOCUS_CONTROL_CAPABILITY+x}" ]
[ -z "${LOCUS_EXECUTOR_CAPABILITY+x}" ]
[ -z "${ROUTE_GITHUB+x}${ROUTE_VERCEL+x}${ROUTE_SUPABASE+x}${ROUTE_AWS+x}" ]
[ -z "${AWS_PROFILE+x}" ]
[ -z "${CROSS_PROVIDER_SETTING+x}" ]
[ -d "$HOME" ]
[ "$USERPROFILE" = "$HOME" ]
case "$GH_CONFIG_DIR" in "$HOME"/*) ;; *) exit 31 ;; esac
case "$AWS_CONFIG_FILE" in "$HOME"/*) ;; *) exit 32 ;; esac
case "$AWS_SHARED_CREDENTIALS_FILE" in "$HOME"/*) ;; *) exit 33 ;; esac
[ ! -e "$HOME/previous-launch" ]
: > "$HOME/previous-launch"
printf '%s\n' "$HOME" >> launch-homes
"#,
        );
        for (key, expected_provider, value) in [
            ("GH_TOKEN", "github", "route-github-secret"),
            ("GITHUB_TOKEN", "github", "route-github-secret"),
            (
                "GITHUB_PERSONAL_ACCESS_TOKEN",
                "github",
                "route-github-secret",
            ),
            ("VERCEL_TOKEN", "vercel", "route-vercel-secret"),
            ("SUPABASE_ACCESS_TOKEN", "supabase", "route-supabase-secret"),
            ("AWS_ACCESS_KEY_ID", "aws", "route-key-id"),
            ("AWS_SECRET_ACCESS_KEY", "aws", "route-aws-secret"),
            ("AWS_SESSION_TOKEN", "aws", "route-session-token"),
        ] {
            if provider == expected_provider {
                script.push_str(&format!("[ \"${{{key}:-}}\" = '{value}' ]\n"));
            } else {
                script.push_str(&format!("[ -z \"${{{key}+x}}\" ]\n"));
            }
        }
        script.push_str("[ \"$#\" = 1 ]\n[ \"$1\" = probe ]\n: > child-verified\n");
        self.install(cli, &script);
    }
}

fn assert_ok(out: &Output) {
    assert!(
        out.status.success(),
        "exit {:?}: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn every_adapter_routes_only_its_credential_with_fresh_private_configuration() {
    let f = Fixture::all();
    f.pin();
    for (cli, provider) in [
        ("gh", "github"),
        ("vercel", "vercel"),
        ("supabase", "supabase"),
        ("aws", "aws"),
    ] {
        f.route_probe(cli, provider);
        for _ in 0..2 {
            assert_ok(&f.run(&["exec", "--for", cli, "--", cli, "probe"]));
            assert!(f.path("project/child-verified").exists());
            std::fs::remove_file(f.path("project/child-verified")).unwrap();
        }
    }
    let homes = std::fs::read_to_string(f.path("project/launch-homes")).unwrap();
    let unique: std::collections::BTreeSet<_> = homes.lines().collect();
    assert_eq!(unique.len(), 8, "each invocation gets a fresh HOME");
    for home in unique {
        assert!(
            !std::path::Path::new(home).exists(),
            "CLI HOME was not cleaned up"
        );
    }
}

#[test]
fn selected_provider_does_not_resolve_unrelated_phantom_reference() {
    let f = Fixture::new(&[
        ("github", "env:ROUTE_GITHUB"),
        ("vercel", "phm:UNRELATED_ROUTE"),
    ]);
    f.pin();
    f.route_probe("gh", "github");
    assert_ok(&f.run(&["exec", "--for", "gh", "--", "gh", "probe"]));
}

#[test]
fn route_disables_inherited_environment_even_for_allowed_placeholders() {
    let f = Fixture::all();
    std::fs::write(
        f.path("locus/config.toml"),
        "[exec]\ninherit_env = [\"VERCEL_TOKEN\", \"AWS_PROFILE\", \"CROSS_PROVIDER_SETTING\"]\n",
    )
    .unwrap();
    f.pin();
    f.route_probe("gh", "github");
    assert_ok(&f.run_env(
        &["exec", "--for", "gh", "--", "gh", "probe"],
        &[
            ("VERCEL_TOKEN", "phm_foreign_placeholder"),
            ("CROSS_PROVIDER_SETTING", "foreign"),
        ],
    ));
}

#[test]
fn credential_locators_cannot_cross_through_runtime_locale_allowlist() {
    let f = Fixture::new(&[
        ("github", "env:LC_SELECTED_TOKEN"),
        ("vercel", "env:LC_FOREIGN_TOKEN"),
        ("supabase", "env:LANG"),
    ]);
    f.pin();
    f.install(
        "gh",
        r#"[ "$GH_TOKEN" = route-github-secret ]
[ "$GITHUB_TOKEN" = route-github-secret ]
[ "$GITHUB_PERSONAL_ACCESS_TOKEN" = route-github-secret ]
[ -z "${LC_SELECTED_TOKEN+x}" ]
[ -z "${LC_FOREIGN_TOKEN+x}" ]
[ -z "${LANG+x}" ]
[ -z "${VERCEL_TOKEN+x}${SUPABASE_ACCESS_TOKEN+x}" ]
: > child-verified"#,
    );
    assert_ok(&f.run_env(
        &["exec", "--for", "gh", "--", "gh"],
        &[
            ("LC_SELECTED_TOKEN", "route-github-secret"),
            ("LC_FOREIGN_TOKEN", "foreign-locale-canary"),
            ("LANG", "foreign-language-canary"),
        ],
    ));
    assert!(f.path("project/child-verified").exists());
}

#[test]
fn matching_absolute_executable_path_is_supported() {
    let f = Fixture::all();
    f.pin();
    f.route_probe("gh", "github");
    let executable = f.path("bin/gh");
    assert_ok(&f.run(&[
        "exec",
        "--for",
        "gh",
        "--expect",
        "route",
        "--",
        executable.to_str().unwrap(),
        "probe",
    ]));
}

#[test]
fn unsupported_cli_and_mismatched_executable_refuse_before_spawn() {
    let f = Fixture::all();
    f.pin();
    f.install("other", "exit 0");
    f.refused(&["exec", "--for", "other", "--", "other"]);
    f.refused(&["exec", "--for", "gh", "--", "other"]);
}

#[test]
fn provider_missing_from_pin_refuses_before_spawn() {
    let f = Fixture::new(&[("github", "env:ROUTE_GITHUB")]);
    f.pin();
    f.install("vercel", "exit 0");
    f.refused(&["exec", "--for", "vercel", "--", "vercel"]);
}

#[test]
fn duplicate_provider_is_ambiguous_and_refused() {
    let f = Fixture::new(&[
        ("github", "env:ROUTE_GITHUB"),
        ("GitHub", "env:ROUTE_VERCEL"),
    ]);
    f.pin();
    f.install("gh", "exit 0");
    f.refused(&["exec", "--for", "gh", "--", "gh"]);
}

#[test]
fn absent_and_empty_credentials_refuse_without_strict_creds() {
    let f = Fixture::new(&[("github", "env:ROUTE_ABSENT")]);
    f.pin();
    f.install("gh", "exit 0");
    f.refused(&["exec", "--for", "gh", "--", "gh"]);
    let out = f.run_env(
        &["exec", "--for", "gh", "--", "gh"],
        &[("ROUTE_ABSENT", "")],
    );
    assert!(!out.status.success());
    assert!(!f.path("project/child-started").exists());
}

#[test]
fn malformed_or_incomplete_aws_bundle_refuses_before_spawn() {
    let f = Fixture::all();
    f.pin();
    f.install("aws", "exit 0");
    for value in [
        "not-json",
        r#"{"AWS_SECRET_ACCESS_KEY":"route-aws-secret"}"#,
        r#"{"AWS_ACCESS_KEY_ID":"","AWS_SECRET_ACCESS_KEY":"route-aws-secret"}"#,
    ] {
        let out = f.run_env(
            &["exec", "--for", "aws", "--", "aws"],
            &[("ROUTE_AWS", value)],
        );
        assert!(!out.status.success());
        assert!(!f.path("project/child-started").exists());
    }
}

#[test]
fn unpinned_route_refuses_before_spawn() {
    let f = Fixture::all();
    f.install("gh", "exit 0");
    f.refused(&["exec", "--for", "gh", "--", "gh"]);
}

#[test]
fn invalid_session_seal_refuses_before_spawn() {
    let f = Fixture::all();
    f.pin();
    f.install("gh", "exit 0");
    let active_path = f.path("locus/sessions/active.json");
    let mut active: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&active_path).unwrap()).unwrap();
    active["seal"] = serde_json::Value::String("0".repeat(64));
    std::fs::write(active_path, serde_json::to_vec(&active).unwrap()).unwrap();
    f.refused(&["exec", "--for", "gh", "--", "gh"]);
}

#[test]
fn no_resolve_cannot_launch_a_provider_route() {
    let f = Fixture::all();
    f.pin();
    f.install("gh", "exit 0");
    f.refused(&["exec", "--for", "gh", "--no-resolve", "--", "gh"]);
}

#[test]
fn expected_alias_and_full_binding_drift_still_gate_route() {
    let f = Fixture::all();
    f.pin();
    f.install("gh", "exit 0");
    f.refused(&["exec", "--for", "gh", "--expect", "different", "--", "gh"]);
    let original = std::fs::read_to_string(f.binding_path()).unwrap();
    // Modify an unselected provider: drift must compare the full binding.
    std::fs::write(
        f.binding_path(),
        original.replace("env:ROUTE_VERCEL", "env:DRIFTED_VERCEL"),
    )
    .unwrap();
    f.refused(&["exec", "--for", "gh", "--", "gh"]);
}

#[test]
fn exact_session_selection_cannot_resolve_provider_credentials() {
    let f = Fixture::all();
    f.pin();
    f.install("gh", "exit 0");
    let active: serde_json::Value =
        serde_json::from_slice(&std::fs::read(f.path("locus/sessions/active.json")).unwrap())
            .unwrap();
    let session_id = active["session_id"].as_str().expect("active session id");
    let out = f.run_env(
        &["exec", "--for", "gh", "--", "gh"],
        &[("LOCUS_SESSION_ID", session_id)],
    );
    assert!(!out.status.success());
    assert!(!f.path("project/child-started").exists());
}

#[test]
fn aws_long_lived_credentials_do_not_inherit_an_ambient_session_token() {
    let f = Fixture::all();
    f.pin();
    f.install(
        "aws",
        r#"[ "$AWS_ACCESS_KEY_ID" = route-key-id ]
[ "$AWS_SECRET_ACCESS_KEY" = route-aws-secret ]
[ -z "${AWS_SESSION_TOKEN+x}" ]"#,
    );
    assert_ok(&f.run_env(
        &["exec", "--for", "aws", "--", "aws"],
        &[
            ("ROUTE_AWS", r#"{"AWS_ACCESS_KEY_ID":"route-key-id","AWS_SECRET_ACCESS_KEY":"route-aws-secret"}"#),
            ("AWS_SESSION_TOKEN", "ambient-session"),
        ],
    ));
}

#[test]
fn provider_child_exit_code_is_preserved() {
    let f = Fixture::all();
    f.pin();
    f.install("gh", "printf '%s' \"$HOME\" > launch-home\nexit 7");
    let out = f.run(&["exec", "--for", "gh", "--", "gh"]);
    assert_eq!(out.status.code(), Some(7));
    let home = std::fs::read_to_string(f.path("project/launch-home")).unwrap();
    assert!(
        !std::path::Path::new(&home).exists(),
        "nonzero exit leaked CLI HOME"
    );
}
