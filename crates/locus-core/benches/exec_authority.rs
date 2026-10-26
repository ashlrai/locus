//! Median cost of one sealed pin check (`Store::require_active`).
//!
//! This is the broker round trip on the `locus exec` path, measured in
//! process. `cargo bench -p locus-core --bench exec_authority` prints it.
//! There is no wall-clock assertion: a loaded machine must not fail CI.
//! The full `locus exec -- true` number is in docs/exec-latency.md.

use std::time::Instant;

use locus_core::{Binding, Store};

const BINDING: &str = r#"
[binding]
id = "bnd_bench"
alias = "bench"
tenant = "bench"

[binding.policy]
default = "allow"
max_ttl = "1h"

[[binding.providers]]
provider = "github"
account = "bench"
credential_ref = "env:LOCUS_BENCH_UNSET"
"#;

fn main() {
    // An inherited executor capability would validate as the child, not as
    // the operator who just pinned. The bench supplies its own home.
    std::env::remove_var("LOCUS_SESSION_ID");
    std::env::remove_var("LOCUS_EXECUTOR_CAPABILITY");

    let dir = tempfile::tempdir().expect("temp home");
    let store = Store::open(dir.path()).expect("open store");
    let binding = Binding::parse_toml(BINDING).expect("binding toml");
    store.save_binding(&binding).expect("save binding");
    store
        .pin("bench", dir.path(), Some("bench".into()), true)
        .expect("pin");

    // Drop the cold start (broker thread, page cache).
    store.require_active().expect("warmup");

    let mut samples = Vec::with_capacity(20);
    for _ in 0..21 {
        let started = Instant::now();
        store.require_active().expect("require_active");
        samples.push(started.elapsed());
    }
    samples.remove(0);
    samples.sort();
    let median = samples[samples.len() / 2];
    let min = samples[0];
    let max = samples[samples.len() - 1];
    println!(
        "exec_authority require_active n={} min={:.3}ms median={:.3}ms max={:.3}ms",
        samples.len(),
        min.as_secs_f64() * 1e3,
        median.as_secs_f64() * 1e3,
        max.as_secs_f64() * 1e3
    );
}
