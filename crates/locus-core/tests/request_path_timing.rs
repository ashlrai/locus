//! Criterion-free request-path microbenchmark. Run explicitly in release mode:
//! cargo test -p locus-core --release --test request_path_timing -- --ignored --nocapture
//! Uses only synthetic metadata, with no credential resolution or network I/O.
use locus_core::seal::SealKey;
use locus_core::session::PinSource;
use locus_core::{Binding, CompositeWorkerManager, Session};
use std::time::Instant;
use tempfile::tempdir;

// Run explicitly in release mode; timings are observations, not CI gates.
#[test]
#[ignore]
fn request_path_timing() {
    use std::hint::black_box;
    let dir = tempdir().unwrap();
    let session = Session::new(
        "bnd_acme",
        "acme",
        "acme-corp",
        None,
        PinSource::Explicit,
        Some("test".into()),
        chrono::Duration::hours(1),
        dir.path().to_str().unwrap().into(),
        &SealKey::generate(),
    );
    let binding = Binding::parse_toml(
        r#"
[binding]
id = "bnd_acme"
alias = "acme"
tenant = "acme-corp"
[[binding.providers]]
provider = "supabase"
account = "acme"
credential_ref = "phm:SUPABASE_ACME"
[binding.providers.scope]
project_ref = "proj_acme"
[[binding.providers]]
provider = "github"
account = "acme-gh"
credential_ref = "phm:GH_ACME"
[binding.providers.scope]
orgs = ["acme-corp"]
"#,
    )
    .unwrap();
    let mut manager = CompositeWorkerManager::with_idle_timeout(None);
    manager.ensure_binding(&session, &binding).unwrap();
    let args = serde_json::json!({});
    let hosts = vec!["gateway.example:443".to_owned()];
    for (label, iterations) in [
        ("composite_synthetic", 10_000),
        ("gateway_validate", 10_000),
    ] {
        let mut samples = Vec::new();
        for sample in 0..11 {
            let start = Instant::now();
            for _ in 0..iterations {
                if label == "composite_synthetic" {
                    black_box(
                        manager
                            .call_tool(&session, &binding, black_box("github.scope"), &args)
                            .unwrap(),
                    );
                } else {
                    black_box(
                        locus_core::gateway::validate_base_url(
                            black_box("https://gateway.example/v1"),
                            &hosts,
                        )
                        .unwrap(),
                    );
                }
            }
            if sample > 0 {
                samples.push(start.elapsed().as_nanos() / iterations);
            }
        }
        samples.sort_unstable();
        println!(
            "{label}: min={} median={} max={} ns/op n=10 iterations={iterations}",
            samples[0], samples[5], samples[9]
        );
    }
}
