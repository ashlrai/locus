//! `locus exec` startup-overhead budget (issue #170).
//!
//! `locus exec` wraps a child process in Locus work: argument handling,
//! store open, pin/seal + authority verification, binding preflight, drift
//! check, credential scrub, executor grant, child spawn, and the post-child
//! audit append. The fixed budget is the median *overhead*:
//!
//! ```text
//! overhead = median(locus exec -- true) - median(locus --version)
//!          - median(direct true)
//! ```
//!
//! so binary startup (dyld + clap) and the machine's own spawn cost cancel
//! and only the work `exec` adds is measured — a dozen milliseconds on a
//! loaded dev host, ~2ms on the reference Linux box. The `audit` phase, read
//! from `LOCUS_EXEC_TRACE` stderr marks, has its own fixed budget: the
//! post-child append is delegated to the authority broker, so the
//! synchronous `fsync` it replaced (the former largest phase, ~4.5–5.8ms on
//! this dev host, ~0.8ms on the reference Linux box) must never reappear on
//! the exec exit path.
//!
//! This is a benchmark gate, not a scheduler-sensitive default unit test:
//! run it on an idle host against an explicit release binary, exactly like
//! `exec_perf::exec_startup_ceiling`:
//!
//! ```sh
//! cargo build --release -p locus-cli
//! LOCUS_EXEC_BENCH_BIN="$PWD/target/release/locus" \
//!   cargo test -p locus-cli --test integration exec_startup_budget -- --ignored --nocapture
//! ```
//!
//! `LOCUS_EXEC_BENCH_BIN` falls back to the cargo-provided binary so the test
//! also runs under `cargo test --release`; never measure a debug build
//! against these budgets.

#![cfg(unix)]

use std::process::{Command, Output};
use std::time::{Duration, Instant};

/// Median locus-added wall clock once binary startup and child spawn are
/// subtracted. ~2ms on the reference Linux host, under a dozen on a loaded
/// dev machine; a new subprocess, poll, or network call on the exec path
/// cannot hide inside it.
const OVERHEAD_BUDGET: Duration = Duration::from_millis(30);
/// Median `audit` phase reported by `LOCUS_EXEC_TRACE`. The broker append
/// round trip is well under a millisecond on an idle host; the synchronous
/// fsync it replaced measured ~4.5–5.8ms on this dev host, so it lands over
/// the budget on the host class it was slowest on.
const AUDIT_PHASE_BUDGET: Duration = Duration::from_millis(4);
const SAMPLES: usize = 25;

struct Fixture {
    home: tempfile::TempDir,
    cwd: tempfile::TempDir,
    cap: String,
    binary: std::path::PathBuf,
}

fn fixture(binary: std::path::PathBuf) -> Fixture {
    let mut f = Fixture {
        home: tempfile::tempdir().unwrap(),
        cwd: tempfile::tempdir().unwrap(),
        cap: String::new(),
        binary,
    };
    let qs = f.locus(&["quickstart"], false);
    assert!(
        qs.status.success(),
        "quickstart failed (never print its output; it can carry capabilities)"
    );
    f.cap = std::fs::read_to_string(f.home.path().join("control_capability"))
        .expect("quickstart persists the control capability")
        .trim()
        .to_string();
    f
}

impl Fixture {
    fn locus(&self, args: &[&str], trace: bool) -> Output {
        let mut cmd = Command::new(&self.binary);
        cmd.args(args)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.home.path())
            .env("LOCUS_HOME", self.home.path())
            .env("NO_COLOR", "1")
            .current_dir(self.cwd.path());
        if trace {
            cmd.env("LOCUS_EXEC_TRACE", "1");
        }
        if !self.cap.is_empty() {
            cmd.env("LOCUS_CONTROL_CAPABILITY", &self.cap);
        }
        // Never print captured output: fixture setup can carry capabilities.
        cmd.output().expect("launch locus binary")
    }
}

/// `locus_exec_phase <name> <ms>` from `LOCUS_EXEC_TRACE` stderr marks.
fn trace_phase_ms(stderr: &[u8], phase: &str) -> Option<Duration> {
    let needle = format!("locus_exec_phase {phase} ");
    String::from_utf8_lossy(stderr)
        .lines()
        .filter_map(|line| line.strip_prefix(&needle))
        .filter_map(|rest| rest.trim().parse::<f64>().ok())
        .next_back()
        .map(|ms| Duration::from_secs_f64(ms / 1000.0))
}

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort();
    let mid = samples.len() / 2;
    if samples.len().is_multiple_of(2) {
        (samples[mid - 1] + samples[mid]) / 2
    } else {
        samples[mid]
    }
}

#[test]
#[ignore = "release wall-clock benchmark; requires an idle host, see module docs"]
fn exec_startup_overhead_budget() {
    let binary = std::env::var_os("LOCUS_EXEC_BENCH_BIN")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(env!("CARGO_BIN_EXE_locus")));
    let binary = std::fs::canonicalize(binary).expect("locus binary must exist");
    let f = fixture(binary);

    // Interleave the three measurements so all samples share machine
    // conditions; iteration 0 is the warmup for every series.
    let mut version = Vec::with_capacity(SAMPLES);
    let mut direct = Vec::with_capacity(SAMPLES);
    let mut exec = Vec::with_capacity(SAMPLES);
    let mut audit = Vec::with_capacity(SAMPLES);
    for iteration in 0..SAMPLES + 1 {
        let started = Instant::now();
        let status = Command::new("/usr/bin/true")
            .status()
            .expect("spawn /usr/bin/true");
        let direct_elapsed = started.elapsed();
        assert!(status.success());

        let started = Instant::now();
        let out = f.locus(&["--version"], false);
        let version_elapsed = started.elapsed();
        assert!(out.status.success());

        let started = Instant::now();
        let out = f.locus(&["exec", "--", "/usr/bin/true"], true);
        let exec_elapsed = started.elapsed();
        assert!(out.status.success(), "exec iteration {iteration} failed");

        if iteration > 0 {
            direct.push(direct_elapsed);
            version.push(version_elapsed);
            exec.push(exec_elapsed);
            audit.push(
                trace_phase_ms(&out.stderr, "audit").expect("LOCUS_EXEC_TRACE audit phase missing"),
            );
        }
    }

    let direct_median = median(direct);
    let version_median = median(version);
    let exec_median = median(exec);
    let audit_median = median(audit);
    let overhead = exec_median.saturating_sub(version_median + direct_median);
    println!(
        "exec_startup_budget n={SAMPLES} direct_median={:.3}ms version_median={:.3}ms \
         exec_median={:.3}ms overhead_median={:.3}ms budget={}ms \
         audit_median={:.3}ms audit_budget={}ms",
        direct_median.as_secs_f64() * 1000.0,
        version_median.as_secs_f64() * 1000.0,
        exec_median.as_secs_f64() * 1000.0,
        overhead.as_secs_f64() * 1000.0,
        OVERHEAD_BUDGET.as_millis(),
        audit_median.as_secs_f64() * 1000.0,
        AUDIT_PHASE_BUDGET.as_millis(),
    );
    assert!(
        overhead < OVERHEAD_BUDGET,
        "locus exec startup overhead {overhead:?} exceeded {OVERHEAD_BUDGET:?} budget"
    );
    assert!(
        audit_median < AUDIT_PHASE_BUDGET,
        "post-child audit phase {audit_median:?} exceeded {AUDIT_PHASE_BUDGET:?} budget — \
         a synchronous fsync may be back on the exec exit path"
    );
}
