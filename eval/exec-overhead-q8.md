# Reproducing `locus exec` overhead

This measures local process wall time for `locus exec --no-resolve -- /usr/bin/true`
against `/usr/bin/true` alone. It needs a Rust toolchain, Python 3 and a Unix-like
host with `/usr/bin/true`; it installs nothing and needs no provider credentials,
network service, or benchmark dependency. Cargo may need network access to fetch
locked build dependencies. The sample binding contains placeholder `env:` references;
`--no-resolve` prevents resolving them. The result covers the local sealed pin,
authority broker, environment isolation, child launch and audit write, not provider
API latency or credential resolution.

Run from the repository root in one Bash process. The broker belongs to the shell
that pins, so do not split setup and measurement across shells. Use a fresh
throwaway home outside any workspace with a `.locus.toml` allowlist. This is a
synthetic local test pin, not an instruction to change a real account pin.

```bash
set -euo pipefail
repo="$PWD"
# Discard inherited Locus session, capability, tracing and credential settings.
while IFS= read -r name; do unset "$name"; done < <(compgen -A variable LOCUS_ || true)
# Keep the existing Rust toolchain available after changing HOME.
export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
export PATH="$CARGO_HOME/bin:$PATH"
export CARGO_TARGET_DIR="$repo/target"
work="$(mktemp -d "${TMPDIR:-/tmp}/locus-q8.XXXXXXXX")"
export HOME="$work/home" LOCUS_HOME="$work/locus"
mkdir -p "$HOME"
cargo build --locked --release -p locus-cli
locus="$repo/target/release/locus"
cd "$work"
"$locus" init --with-samples
# The hook loads this temporary home's control capability; pin starts the broker.
eval "$("$locus" hook bash)"
"$locus" pin personal
"$locus" exec --no-resolve -- /usr/bin/true
python3 - "$locus" <<'PY'
import statistics
import subprocess
import sys
import time

locus = sys.argv[1]
commands = {
    "direct": ["/usr/bin/true"],
    "locus": [locus, "exec", "--no-resolve", "--", "/usr/bin/true"],
}
samples = {name: [] for name in commands}
# Alternate order to reduce ordering bias. Drop the first pair as warm-up.
for i in range(21):
    names = list(commands) if i % 2 == 0 else list(reversed(commands))
    for name in names:
        start = time.perf_counter_ns()
        result = subprocess.run(commands[name], stdin=subprocess.DEVNULL,
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        elapsed = (time.perf_counter_ns() - start) / 1_000_000
        if result.returncode != 0:
            raise SystemExit(f"{name} run {i + 1} failed: exit {result.returncode}; no result")
        if i:
            samples[name].append(elapsed)
for name, values in samples.items():
    print(f"{name}: n={len(values)} min={min(values):.3f} "
          f"median={statistics.median(values):.3f} max={max(values):.3f} ms")
print(f"difference of medians: {statistics.median(samples['locus']) - statistics.median(samples['direct']):.3f} ms")
PY
"$locus" leave
printf 'Temporary files: %s\n' "$work"
```

The script aborts on setup or child failure rather than reporting a fast failure as
performance. Inspect errors from the smoke command before retrying. The temporary
directory contains local session and audit state; delete it when finished. No
real `~/.locus` is used. Run on an otherwise idle host, repeat whole fresh-home
runs, and record the commit (`git rev-parse HEAD`), OS/architecture, Rust version,
build profile, filesystem, output and whether the host was loaded. Compare builds
on the same host with the same procedure; report the distribution and difference
of medians, not a universal performance guarantee. Python startup and measurement
cost, scheduling, filesystem caching and audit sync affect results. Redirecting
output to `/dev/null` excludes terminal rendering and differs from the inherited
output in [the existing harness](../scripts/bench_exec.py) and its historical
[latency report](../docs/exec-latency.md). Neither comparison isolates individual
internal phases; no fixed pass/fail latency threshold is implied.
