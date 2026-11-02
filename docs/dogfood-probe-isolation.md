# Dogfood probe root isolation

[`scripts/dogfood.sh`](../scripts/dogfood.sh) runs two self-contained probes,
each under its own temporary root in `${TMPDIR:-/tmp}`: the upstream worker
sandbox attestation (`sandbox_attestation()`, step 1b) and the optional
multi-tenant grant probe (`mt_grant_probe()`, step 8b, `DOGFOOD_MT=1`). The
roots isolate probe state from the main dogfood home, from each other, from
the caller's environment, and from the operator's real `~/.locus`. Neither
probe depends on `DOGFOOD_USE_REAL_HOME`; both always allocate a throwaway
root.

## Creation

| Probe root | Function | Allocation |
| --- | --- | --- |
| `locus-sandbox-dogfood.XXXXXX` | `sandbox_attestation()` | `mktemp -d` assigns `probe_root` before the function enters its subshell. A failed allocation returns 1 before any probe state exists; the caller reports `upstream sandbox attestation failed`. |
| `locus-mt-dogfood.XXXXXX` | `mt_grant_probe()` | `mktemp -d` assigns `probe_root` inside the probe subshell. A failed allocation exits the subshell with status 1 before `locus init` or server startup; the caller reports `multi-tenant probe failed`. |

Both templates honor `TMPDIR`; an unset or empty value selects `/tmp`, and a
nonexistent parent fails the allocation (see
[temporary-directory failure handling](dogfood-temp-failures.md)).

## Use

Each probe root holds a complete, disposable Locus context:

- **Private home** — both functions `export LOCUS_HOME="$probe_root/home"`,
  so `locus init`, `locus binding add`, and the pin write bindings, session
  state, and grant rosters under the probe root only.
- **Cleared session** — both `unset LOCUS_SESSION_ID`, so an inherited
  session id cannot smuggle the probes into someone else's pin.
- **Synthetic bindings** — `sandbox-probe` (credential ref
  `env:LOCUS_DOGFOOD_SANDBOX_UNUSED`, never resolved) and `mt-probe`
  (`env:LOCUS_DOGFOOD_MT_UNUSED`) are distinct from the main flow's `dogfood`
  binding and from any operator binding.
- **Subshell scoping** — each probe body runs in `( … )`, so `LOCUS_HOME`,
  `cd`, and trap changes never propagate back to the caller. The regression
  suite asserts the caller's `PWD`, `LOCUS_HOME`, and `LOCUS_SESSION_ID` are
  unchanged after `mt_grant_probe` returns.
- **Working-directory fence** — `mt_grant_probe()` runs `cd "$probe_root"`
  so a caller's workspace `.locus.toml` allowlist cannot deny the `mt-probe`
  pin at mint time; `sandbox_attestation()` runs `cd "$probe_root/project"`.

Inside its root, `sandbox_attestation()` writes the `sandbox-marker-mcp`
fixture, enables `[binding.providers.upstream]` with `resolve_secrets =
false` and `sandbox = true`, pins `sandbox-probe`, and drives JSON-RPC through
`locus exec --no-resolve -- "$MCP_BIN"`; captured stdout/stderr stay under
the root. The fixture performs no credential lookup or network I/O.
`mt_grant_probe()` starts `"$MCP_BIN" --http 127.0.0.1:<port>
--multi-tenant` with its own `LOCUS_MCP_HTTP_TOKEN`, logs to
`$probe_root/mt-mcp.log`, then walks mint → roster check → tenantless `401
invalid_grant` → granted `200` → `locus mcp revoke` → `401` → roster empty.
Both probes grep output for credential locators and fail before printing
anything that contains one.

## Cleanup

| Probe root | Cleanup path |
| --- | --- |
| `locus-sandbox-dogfood.XXXXXX` | The subshell copies the root into `sandbox_cleanup_root` — function locals may be out of scope when an `EXIT` trap runs after exit — then installs `trap 'rm -rf "$sandbox_cleanup_root"' EXIT`. |
| `locus-mt-dogfood.XXXXXX` | `trap 'mt_cleanup' EXIT`; `mt_cleanup()` sends `kill` to the recorded server PID when set, `wait`s for it, runs `cd /` so removal does not delete the process's own working directory, then `rm -rf "$probe_root"`. Kill, wait, and `cd` errors are ignored. |

The traps fire on normal return and on trappable failures, including early
`exit` after the trap is installed. Explicit grant revocation is part of the
normal `mt_grant_probe()` sequence, not its trap: an early failure can skip
revocation, but cleanup still removes the isolated home holding grant state.
Neither trap touches the main dogfood home (removed by the caller's own
`cleanup()` trap), a custom `DOGFOOD_PACK`, or the neighboring probe root.
Traps do not cover uncatchable termination or a failed `rm`; inspect leftover
directories before deleting them.

## Readiness effect

Step 1b is a hard gate: `SANDBOX_OK` must be 1 for `readiness_gate()` to
print `DOGFOOD READY` (exit 2 — an attested fail-closed refusal on an
unsupported platform — still blocks readiness). Step 8b is hard only when
`DOGFOOD_MT=1`; by default the probe does not run at all.

## Related

- [Probe directory roles](dogfood-probe-roles.md) — what each root contains
- [Probe cleanup](dogfood-probe-cleanup.md) — trap order and limitations
- [Temporary-directory prefixes](dogfood-temp-prefix-reference.md) — template inventory
- [Temporary-directory failures](dogfood-temp-failures.md) — allocation failure paths

Run `node --test scripts/dogfood.test.mjs` for the probe isolation and
cleanup regression checks.
