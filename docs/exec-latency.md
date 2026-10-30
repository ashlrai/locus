# `locus exec` latency

## Reproducing issue #170

This procedure for [issue #170](https://github.com/ashlrai/locus/issues/170)
measures local process wall time for `locus exec --no-resolve -- /usr/bin/true`
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
build profile, filesystem, output and whether the host was loaded. For each run,
retain both min/median/max ranges and the difference of medians. Compare these
across fresh-home runs to see variation; for changes, repeat both builds on the
same host with the same procedure and compare their run-to-run medians. Do not
interpret small differences within that variation as a demonstrated improvement.
For a same-host comparison with direnv and sops, use the procedure below.
Process startup, measurement overhead,
scheduling, filesystem caching and audit sync affect results. Redirecting output
to `/dev/null` excludes terminal rendering and differs from the inherited output
in [the historical harness](../scripts/bench_exec.py) and the results below.
Neither comparison isolates individual internal phases; no fixed pass/fail latency
threshold is implied.

## Comparing launchers for issue #170

This measures the same `/usr/bin/true` child through all three launchers, plus
the direct child as a baseline. It compares **launch paths**, not equivalent
security guarantees: Locus verifies a live pin, isolates the child and writes an
audit record and resolves a synthetic `env:` credential; direnv evaluates an
allowed `.envrc`; sops decrypts an age-encrypted dotenv file. Each launcher
injects the same synthetic `RESEND_API_KEY` value into the child; the direct
baseline does not inject it. The timed child does not inspect the value. Setup,
encryption, approval and pinning are outside the timed region. No network or real credentials
are needed after the prerequisites are installed.

Prerequisites: Bash, Python 3, a Rust toolchain, `direnv`, `sops`, `age-keygen`,
and `/usr/bin/true` on a Unix-like host. Run the entire block from the repository
root in one Bash process, on an idle host. It builds the locked release binary,
uses a fresh temporary home outside the workspace and removes it on exit. The
generated age identity is disposable; never use a real secret in the fixture.

```bash
set -euo pipefail
repo="$PWD"
while IFS= read -r name; do unset "$name"; done < <(compgen -A variable LOCUS_ || true)
while IFS= read -r name; do unset "$name"; done < <(compgen -A variable SOPS_ || true)
while IFS= read -r name; do unset "$name"; done < <(compgen -A variable DIRENV_ || true)
unset RESEND_API_KEY
export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
export PATH="$CARGO_HOME/bin:$PATH" CARGO_TARGET_DIR="$repo/target"
work="$(mktemp -d "${TMPDIR:-/tmp}/locus-peers.XXXXXXXX")"
# direnv must allow and execute the same physical path (for example on macOS /tmp).
work="$(cd "$work" && pwd -P)"
trap 'rm -rf "$work"' EXIT
export HOME="$work/home" LOCUS_HOME="$work/locus"
export SOPS_AGE_KEY_FILE="$work/age-key.txt" SOPS_DISABLE_VERSION_CHECK=1
export DIRENV_LOG_FORMAT=
mkdir -p "$HOME" "$work/direnv"
cargo build --locked --release -p locus-cli
locus="$repo/target/release/locus"
cd "$work"
"$locus" init
eval "$("$locus" hook bash)"
"$locus" binding add bench --tenant bench --provider resend --account bench \
  --credential-ref env:LOCUS_BENCH_FIXTURE --non-interactive
export LOCUS_BENCH_FIXTURE=fixture
"$locus" pin bench
printf 'export RESEND_API_KEY=fixture\n' > "$work/direnv/.envrc"
direnv allow "$work/direnv/.envrc"
printf 'RESEND_API_KEY=fixture\n' > "$work/plain.env"
age-keygen -o "$SOPS_AGE_KEY_FILE" 2>/dev/null
recipient="$(age-keygen -y "$SOPS_AGE_KEY_FILE")"
sops --encrypt --age "$recipient" --input-type dotenv --output-type dotenv \
  "$work/plain.env" > "$work/encrypted.env"
rm "$work/plain.env"
# Check the same synthetic value reaches each child without printing it.
"$locus" exec --strict-creds -- /bin/sh -c 'test "$RESEND_API_KEY" = fixture'
direnv exec "$work/direnv" /bin/sh -c 'test "$RESEND_API_KEY" = fixture'
sops exec-env "$work/encrypted.env" 'test "$RESEND_API_KEY" = fixture'
python3 - "$locus" "$work/direnv" "$work/encrypted.env" <<'PY'
import statistics
import subprocess
import sys
import time

locus, directory, encrypted = sys.argv[1:]
commands = {
    "direct": ["/usr/bin/true"],
    "locus": [locus, "exec", "--strict-creds", "--", "/usr/bin/true"],
    "direnv": ["direnv", "exec", directory, "/usr/bin/true"],
    "sops": ["sops", "exec-env", encrypted, "/usr/bin/true"],
}
samples = {name: [] for name in commands}
names = list(commands)
warmups, runs = 4, 32
for i in range(warmups + runs):
    # Rotate the first command, including during warmup, to balance order.
    for name in names[i % len(names):] + names[:i % len(names)]:
        start = time.perf_counter_ns()
        result = subprocess.run(commands[name], stdin=subprocess.DEVNULL,
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        elapsed = (time.perf_counter_ns() - start) / 1_000_000
        if result.returncode != 0:
            raise SystemExit(f"{name} run {i + 1} failed: exit {result.returncode}; no results")
        if i >= warmups:
            samples[name].append(elapsed)
for name, values in samples.items():
    print(f"{name}: n={len(values)} min={min(values):.3f} "
          f"median={statistics.median(values):.3f} max={max(values):.3f} ms")
PY
"$locus" leave
```

All four warmup rounds are discarded; each subsequent round measures every
command once, with balanced starting order. A nonzero exit aborts without
printing statistics. To diagnose a failure, run the relevant invocation outside
the timer with the same temporary setup; never print the key or environment.
Repeat the complete block in fresh homes and record each run's ranges and medians,
the commit, tool versions, OS/architecture, filesystem and host load. Output is
redirected during timing, so terminal rendering is excluded; scheduling, caches,
audit sync, process startup and filesystem behavior remain included. Compare
within-run medians and variation across runs, without treating a small difference
as meaningful if it falls within that variation. These instructions make no
claim about results on a machine where they have not been measured.

## Historical measurement

Measured on 2026-10-25 ET (2026-10-26 UTC) on one x86_64 Linux machine (kernel 6.12.94+, rustc
1.99.1, release build). 21 runs, first dropped. Wall clock is
`time.perf_counter` around the process. `ru_maxrss` comes from `os.wait4`
and is kilobytes on this Linux. stdin is `/dev/null`. stdout and stderr are
inherited, so the human banner is included. The pin is live in the same
shell (`locus quickstart` on a fresh `LOCUS_HOME`, sample bindings only, no
real credentials). Harness: [scripts/bench_exec.py](../scripts/bench_exec.py).

The before binary is parent commit `96c4a67`. The after binary is this
change. Both were measured in that harness on this machine, in separate
fresh homes.

`LOCUS_EXEC_TRACE=1` prints phase timings on stderr (`locus_exec_phase`).
It is off by default. Those runs are not the harness median: piping stderr
drops the cost of writing the human banner to a terminal.

## Harness medians

| Command | Before min / median / max (s) | After min / median / max (s) | After `ru_maxrss` median (KiB) |
|---|---|---|---|
| `locus whoami` | 0.019870 / 0.020237 / 0.020765 | 0.002419 / 0.003121 / 0.003877 | 8646 |
| `locus exec -- true` | 0.048890 / 0.050704 / 0.051874 | 0.003255 / 0.004206 / 0.005101 | 9106 |
| `locus status --oneline` | 0.020038 / 0.020251 / 0.020469 | 0.002373 / 0.002659 / 0.003342 | 8578 |

Before `ru_maxrss` medians were 8760 KiB (`whoami`), 9182 KiB (`exec`), and
8688 KiB (`status`). Every run exited 0. The after column is the final
binary, including the 10 ms idle accept slice.

Peer medians recorded in PR #165 on a similar host, not re-run here:
`direnv exec` 0.016 s, `sops exec-env` 0.011 s. `locus exec -- true` on this
change is 0.004 s (median 0.004206 s).

## What the time was

One `strace -tt -T` of `locus exec -- true` on the before binary, pin live.
The client opened five Unix connections to the authority broker. `connect`
itself was about 0.01 ms. Attaching `strace` to the broker showed
`clock_nanosleep` of 10.1 to 10.4 ms after every `accept` that returned
`EAGAIN`. Each new connection waited out that sleep. Times below are from
the client's `execve` of `locus` to the named syscall. strace inflates the
total (about 62 ms to the `fsync` return) relative to the harness median of
0.051 s. The shape is the same: five broker waits.

| Phase (before, one traced run) | Time from start | Delta |
|---|---|---|
| Args, store open, seal key, read `active.json`, first broker connect | 5.8 ms | 5.8 ms |
| Second broker validate (`require_active` after the delegated check) | 16.3 ms | 10.5 ms |
| Load binding, re-read the session, third broker validate (drift) | 26.9 ms | 10.6 ms |
| Reload binding, read `config.toml` twice, `BeginControl` | 36.0 ms | 9.1 ms |
| `GrantExecutor` | 44.5 ms | 8.5 ms |
| Scrub, secret resolution, `execve` of `true` | 56.0 ms | 11.5 ms |
| Audit append and `fsync` (the `fsync` call itself was 1.38 ms) | 61.8 ms | 5.8 ms |

## What changed

The broker waits on `poll` until a client connects, the supervisor check
is due (still 250 ms), or 10 ms have passed so retirement and home deletion
are noticed. A client wakes `poll` immediately, so the idle slice is not
added to a request. A traced after broker showed `poll` waking in about
1 ms, three successful accepts, and no `clock_nanosleep`. The seal, the
broker generation check, and the executor grant stay. On the healthy
operator path the seal and one broker validate happen once, then the grant
(two more broker round trips, same challenge protocol): three connections,
confirmed with `strace`. Drift compares the binding just loaded for this
exec to the pin fingerprint. A file edit after that load is caught on the
next exec. A drift that would freeze asks the broker again before it
writes. `--expect` still refuses before the child starts. The audit `fsync`
stays on this write.

`cargo bench -p locus-core --bench exec_authority` prints the median of
`Store::require_active` in process. It does not fail CI. On this machine
that median was 0.099 ms (n=20, min 0.090 ms, max 0.403 ms).

## After phases

`LOCUS_EXEC_TRACE=1`, 21 runs, first dropped, stderr piped, exits 0.
Medians in milliseconds. The sum of the phase medians is 2.43 ms. The
traced total median is 2.542 ms. The harness median is 4.206 ms because it
inherits the terminal and pays for the banner. Audit `fsync` moves around
on this disk (0.251 ms to 1.321 ms in this sample).

| Phase | Median (ms) | Min / max (ms) | What it includes |
|---|---|---|---|
| `args` | 0.326 | 0.279 / 0.422 | Process start through argument checks |
| `store_open` | 0.066 | 0.055 / 0.119 | `Store::open` (home, directories, seal key) |
| `pin_seal_broker` | 0.323 | 0.226 / 0.697 | Seal HMAC and one broker validate |
| `binding_preflight` | 0.096 | 0.071 / 0.144 | Load the binding and the credential preflight |
| `drift` | 0.028 | 0.021 / 0.043 | Fingerprint compare. No broker call when nothing drifted |
| `scrub_resolve` | 0.056 | 0.047 / 0.086 | Ambient scrub and secret resolution |
| `executor_grant` | 0.209 | 0.179 / 0.614 | Broker `BeginControl` and `GrantExecutor` |
| `spawn_wait` | 0.525 | 0.491 / 0.744 | Spawn and wait, including the stderr banner write into the pipe |
| `audit` | 0.798 | 0.251 / 1.321 | Append the audit line and `fsync` |
| `total` | 2.542 | 1.747 / 3.423 | Process start through the audit mark |
