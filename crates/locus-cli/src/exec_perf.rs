//! Opt-in release benchmark (includes process startup, child exit and the
//! broker-delegated post-child audit append).
//!
//! Build `cargo build --release -p locus-cli`, then run with a temporary HOME
//! and the real CARGO_HOME/RUSTUP_HOME:
//! `LOCUS_EXEC_BENCH_BIN="$PWD/target/release/locus" cargo test -p locus-cli
//! --bin locus exec_perf::exec_startup_ceiling -- --ignored --nocapture`.
//! Use an explicit binary so debug/test builds and ambient PATH cannot silently
//! change the measurement. The fixed ceiling applies to each 30-run median;
//! one warmup is excluded, but every invocation must succeed. Run on an idle
//! host: this is a benchmark gate, not a scheduler-sensitive default unit test.

use locus_core::adapter_registry::{
    ed25519_public_key_b64, parse_ed25519_signing_key, sign_entry_material_ed25519,
};
use locus_core::binding::UpstreamSpec;
use locus_core::marketplace::CommunityAdapterManifest;
use std::path::Path;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

const MEDIAN_CEILING: Duration = Duration::from_millis(40);

struct Fixture {
    home: tempfile::TempDir,
    binary: std::path::PathBuf,
    capability: String,
}

impl Fixture {
    fn new(binary: &Path, community: bool) -> Self {
        let mut fixture = Self {
            home: tempfile::tempdir().unwrap(),
            binary: binary.to_owned(),
            capability: String::new(),
        };
        assert!(
            fixture.run(&["init"]).status.success(),
            "fixture init failed"
        );
        fixture.capability =
            std::fs::read_to_string(fixture.locus_home().join("control_capability"))
                .unwrap()
                .trim()
                .to_owned();

        let mut binding = locus_core::Binding::parse_toml(
            "[binding]\nid='bnd_bench'\nalias='bench'\ntenant='bench'\n\
             [[binding.providers]]\nprovider='github'\naccount='bench'\n\
             credential_ref='env:LOCUS_BENCH_FIXTURE'\n",
        )
        .unwrap();
        if community {
            // Public, deterministic test key; no real provider credentials.
            let signing = parse_ed25519_signing_key(&"07".repeat(32)).unwrap();
            locus_core::add_ed25519_trust_key(
                &fixture.locus_home(),
                "bench-publisher",
                &ed25519_public_key_b64(&signing.verifying_key()),
            )
            .unwrap();
            let mut manifest: CommunityAdapterManifest =
                serde_json::from_value(serde_json::json!({
                    "manifest_version": locus_core::marketplace::COMMUNITY_MANIFEST_VERSION,
                    "entry": {
                        "id": "bench", "name": "Benchmark provider", "status": "community",
                        "synthetic": false, "tools": ["bench.identity"],
                        "signed_by": "bench-publisher"
                    },
                    "upstream": {"command": "locus-bench-upstream-must-not-start"},
                    "credential_env": "BENCH_API_KEY",
                    "publisher": "Benchmark", "version": "1.0.0"
                }))
                .unwrap();
            manifest.entry.signature = Some(sign_entry_material_ed25519(
                &manifest.signing_material(),
                &signing,
            ));
            let mut upstream: UpstreamSpec = manifest.upstream.clone().unwrap();
            upstream.community_adapter = Some(Box::new(manifest));
            binding.providers[0].provider = "bench".into();
            binding.providers[0].upstream = Some(upstream);
        }
        std::fs::write(
            fixture.locus_home().join("bindings/bench.toml"),
            binding.to_toml().unwrap(),
        )
        .unwrap();
        assert!(
            fixture.run(&["pin", "bench"]).status.success(),
            "fixture pin failed"
        );
        fixture
    }

    fn locus_home(&self) -> std::path::PathBuf {
        self.home.path().join("locus")
    }

    fn run(&self, args: &[&str]) -> Output {
        let mut command = Command::new(&self.binary);
        command
            .args(args)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.home.path())
            .env("LOCUS_HOME", self.locus_home())
            .env("NO_COLOR", "1")
            .env("LOCUS_BENCH_FIXTURE", "synthetic-benchmark-value")
            .current_dir(self.home.path());
        if !self.capability.is_empty() {
            command.env("LOCUS_CONTROL_CAPABILITY", &self.capability);
        }
        // Never print captured output: fixture setup can contain capabilities.
        command.output().expect("launch explicit benchmark binary")
    }
}

#[test]
#[ignore = "release wall-clock benchmark; requires LOCUS_EXEC_BENCH_BIN and an idle host"]
fn exec_startup_ceiling() {
    let binary = std::env::var_os("LOCUS_EXEC_BENCH_BIN")
        .expect("set LOCUS_EXEC_BENCH_BIN to the release locus binary");
    let binary = std::fs::canonicalize(binary).expect("benchmark binary must exist");
    for community in [false, true] {
        let fixture = Fixture::new(&binary, community);
        let mut samples = Vec::with_capacity(30);
        for iteration in 0..31 {
            let started = Instant::now();
            let output = fixture.run(&["exec", "--strict-creds", "--", "/usr/bin/true"]);
            let elapsed = started.elapsed();
            assert!(output.status.success(), "exec iteration {iteration} failed");
            if iteration > 0 {
                samples.push(elapsed);
            }
        }
        samples.sort();
        let median = (samples[14] + samples[15]) / 2;
        println!(
            "exec community={community} n=30 min={:.3}ms median={:.3}ms max={:.3}ms ceiling=40ms",
            samples[0].as_secs_f64() * 1000.0,
            median.as_secs_f64() * 1000.0,
            samples[29].as_secs_f64() * 1000.0,
        );
        assert!(median < MEDIAN_CEILING, "exec startup exceeded 40ms median");

        // Each exec must observe current publisher trust. Strict mode must
        // refuse before launching a child when that trust is revoked.
        if community {
            std::fs::remove_file(locus_core::adapter_trust_keys_path(&fixture.locus_home()))
                .unwrap();
            let output = fixture.run(&[
                "exec",
                "--strict-creds",
                "--",
                "/usr/bin/touch",
                "child-ran",
            ]);
            assert!(!output.status.success(), "revoked publisher was accepted");
            assert!(!fixture.home.path().join("child-ran").exists());
        }
    }
}
