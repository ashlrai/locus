//! Regression #173: a configured ref and a locally available credential are
//! separate facts. Neither human nor JSON identity output exposes the locator
//! or value, and unsupported Phantom refs never imply credential availability.

use serde_json::Value;
use std::process::{Command, Output};

const ENV_NAME: &str = "LOCUS_WHOAMI_FIXTURE_CREDENTIAL";
const SYNTHETIC_VALUE: &str = "whoami-synthetic-canary-value";

struct Operator {
    home: tempfile::TempDir,
    project: tempfile::TempDir,
}

impl Operator {
    fn run_output(&self, args: &[&str], value: Option<&str>) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_locus"));
        command
            .args(args)
            .env_clear()
            .env("HOME", self.home.path())
            .env("LOCUS_HOME", self.home.path().join("locus"))
            .env("PATH", self.project.path())
            .env("NO_COLOR", "1")
            .current_dir(self.project.path());
        if let Ok(capability) =
            std::fs::read_to_string(self.home.path().join("locus/control_capability"))
        {
            command.env("LOCUS_CONTROL_CAPABILITY", capability.trim());
        }
        if let Some(value) = value {
            command.env(ENV_NAME, value);
        }
        let output = command.output().expect("locus runs");
        for bytes in [&output.stdout, &output.stderr] {
            let text = String::from_utf8_lossy(bytes);
            assert!(!text.contains(SYNTHETIC_VALUE), "credential value leaked");
            assert!(!text.contains(ENV_NAME), "credential locator leaked");
            assert!(
                !text.contains("PHANTOM_WHOAMI_LOCATOR"),
                "vault locator leaked"
            );
        }
        output
    }

    fn run(&self, args: &[&str], value: Option<&str>) -> Output {
        let output = self.run_output(args, value);
        assert!(output.status.success(), "locus command failed: {args:?}");
        output
    }

    fn add(&self, alias: &str, reference: &str) {
        self.run(
            &[
                "binding",
                "add",
                alias,
                "--tenant",
                "fixture",
                "--provider",
                "github",
                "--account",
                "fixture",
                "--credential-ref",
                reference,
                "--non-interactive",
            ],
            None,
        );
    }
}

#[test]
fn whoami_distinguishes_configured_unset_empty_and_available_credentials() {
    let operator = Operator {
        home: tempfile::tempdir().unwrap(),
        project: tempfile::tempdir().unwrap(),
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let probe = operator.project.path().join("phantom");
        std::fs::write(&probe, "#!/bin/sh\n: > probe-ran\nexit 1\n").unwrap();
        std::fs::set_permissions(probe, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    operator.run(&["init"], None);
    operator.add("environment", &format!("env:{ENV_NAME}"));
    operator.add("vault", "phm:PHANTOM_WHOAMI_LOCATOR");
    operator.run(&["enter", "environment"], None);

    for value in [None, Some(""), Some(SYNTHETIC_VALUE)] {
        let available = value.is_some_and(|value| !value.is_empty());
        let output = operator.run(&["whoami", "--json"], value);
        let document: Value = serde_json::from_slice(&output.stdout).unwrap();
        let credential = &document["providers"][0]["credential"];
        assert_eq!(credential["source"], "environment");
        assert_eq!(credential["reference_present"], true);
        assert_eq!(credential["present"], available);
        assert!(!operator.project.path().join("probe-ran").exists());

        let human = operator.run(&["whoami"], value);
        let text = String::from_utf8(human.stdout).unwrap();
        assert!(text.contains("reference=configured"));
        assert!(text.contains(if available {
            "available=yes"
        } else {
            "available=no"
        }));

        #[cfg(unix)]
        if !available {
            let refused = operator.run_output(
                &[
                    "exec",
                    "--strict-creds",
                    "--",
                    "/bin/sh",
                    "-c",
                    ": > child-ran",
                ],
                value,
            );
            assert_eq!(refused.status.code(), Some(1));
            assert!(String::from_utf8_lossy(&refused.stderr).contains("--strict-creds"));
            assert!(!operator.project.path().join("child-ran").exists());
        }
    }

    operator.run(&["enter", "vault"], None);
    let output = operator.run(&["whoami", "--json"], Some(SYNTHETIC_VALUE));
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    let credential = &document["providers"][0]["credential"];
    assert_eq!(credential["source"], "phantom");
    assert_eq!(credential["reference_present"], true);
    assert_eq!(credential["present"], false);
    assert!(!operator.project.path().join("probe-ran").exists());
}
