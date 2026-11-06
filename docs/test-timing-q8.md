# Q8 test timing

Measured on Apple Silicon macOS with rustc/Cargo 1.95.0 (stable) and cargo-nextest
0.9.150. Both runs use the same worktree, default debug test profile, nextest's
default concurrency, fresh temporary `HOME`, and the real toolchain/cache paths.
No real Locus home or provider credentials are needed.

## Timing method

Run from the workspace root (keep raw results outside the checkout):

```bash
export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
test_home=$(mktemp -d)
HOME="$test_home" NEXTEST_EXPERIMENTAL_LIBTEST_JSON=1 \
  cargo nextest run --workspace --no-fail-fast \
  --message-format libtest-json-plus > /tmp/locus-q8-results.jsonl \
  2> /tmp/locus-q8-results.log
```

Rank binaries by the sum of `exec_time` for their completed test events. In
`libtest-json-plus`, the part of a test's `name` before `$` identifies the binary.
These are aggregate test seconds, not elapsed CI seconds: tests overlap. Include
failed tests, exclude ignored events, and do not include compilation. Nextest
can emit multiple suite summaries for one binary; sum rather than overwrite.

The first nextest build took 22 minutes 19 seconds, excluded below. A local
matching dependency cache was copied into this worktree and checked by Cargo.
Other jobs were active on this host, so these single-run measurements include
scheduler and filesystem noise; they are observations, not performance gates.

## Changes

- Core unit tests reuse one read-only Git repository for all 20 first-writer
  races, retaining a fresh history home, eight writers and every assertion on
  every attempt. This removes 19 Git initializations and remote additions.
  Idle reaping ages the existing fixture's timestamps instead of sleeping 40 ms.
- CLI integration tests bootstrap authority once per five-client matrix,
  removing eight quickstarts across the two matrices. Config paths remain
  distinct, and merge, idempotence and malformed-input assertions all remain.
  The snapshot mutation matrix compiles the exact documented Python code once
  and executes it with fresh globals, instead of starting another interpreter
  for every snapshot. The separate end-to-end documentation tests still run
  the copied shell procedure.
- MCP protocol tests send notifications and subsequent requests on the same
  ordered pipe without the per-handshake 20 ms pause. The invalid-route matrix
  reuses its authority fixture, creates a fresh binding and client for each
  case, and explicitly leaves each pin. Worker-not-started and fail-closed
  assertions remain.

No production logic, security parameters, protected files, test case counts or
assertions changed. No repository-scanning test was added.

## Results

The three slowest binaries in the baseline, in aggregate test seconds:

| Binary | Before (s) | After (s) |
|---|---:|---:|
| `locus-core::locus_core` | 2828.622 | 2092.996 |
| `locus-cli::integration` | 1603.662 | 1427.931 |
| `locus-mcp::mcp_protocol` | 481.192 | 813.008 |

The edited scenarios within those binaries, from the same two full runs:

| Edited scenarios | Before (s) | After (s) |
|---|---:|---:|
| Core first-writer race + idle expiry | 24.193 | 3.379 |
| CLI client-shape matrices + snapshot mutations | 139.101 | 16.135 |
| MCP invalid-route matrix | 19.607 | 18.611 |

All edited scenarios improved. The MCP binary's total increased: unchanged
cases also became much slower as host load shifted. These runs establish the
removed fixture work and observed scenario improvements, but do not establish
a whole-binary MCP speedup or attribute all aggregate changes to this patch.
A quiet-runner repeat is needed for a causal CI speedup claim.


## Validation

The full default-concurrency run was 442.397 s before and 505.413 s after
(excluding builds). Both ran 1,461 tests with the same three existing skips.
Before: 1,455 passed / 6 failed. After: 1,457 passed / 4 failed. The unchanged
property-test straggler and host contention limit wall-time comparisons.
Every edited scenario passed in the final full run. A separate four-thread
focused run passed 55 / 56 tests; its sole failure was the pre-existing MCP
sandbox test. The full run covered every non-ignored test filtered out of
this focused run.

The four final failures also failed before this patch:

- `authority_anchor::tests::start_after_exiting_lock_holder_does_not_wait_out_the_timeout`
- `workers::sandbox::tests::available_npx_keeps_canonical_provenance_inside_real_sandbox`
- `workers::sandbox::tests::outbound_network_policy_allows_provider_connections`
- `mcp_protocol::broker_session_gates_sandbox_start_and_scopes_each_provider_environment`

Two exploratory after-runs were interrupted (630 passed / 52 failed and
104 passed / 4 failed) amid authority startup timeouts. They are excluded from
the tables; the final full run above covers every non-ignored test. No test or
security timeout was weakened to accommodate this host.

Workspace `cargo check --workspace --all-targets --locked`,
`cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`
and `git diff --check` all passed. No commit or push was made.
