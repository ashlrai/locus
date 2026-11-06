# E2E temporary directory types

[`scripts/e2e.sh`](../scripts/e2e.sh) creates the following directories with
`mktemp -d`. Each prefix is followed by six characters chosen by `mktemp`.
These are script variables, not options for selecting the directory names.

| Variable | Prefix | Purpose |
| --- | --- | --- |
| `LOCUS_HOME` | `locus-e2e.` | Main isolated Locus home for the suite and its fixtures. |
| `WS_DIR` | `locus-ws.` | Workspace used to exercise binding allowlist enforcement. |
| `GRAPH_HOME` | `locus-graph-imp.` | Optional fresh Locus home for the graph import fallback. |
| `SECURITY_HOME` | `locus-security-e2e.` | Isolated home for security probes; also created in a cleanup-check subshell. |
| `SECURITY_WS` | `locus-security-ws.` | Workspace for the adversarial security probes. |

All five are created directly under `TMPDIR` when it is nonempty, or under
`/tmp` when it is unset or empty. A supplied `TMPDIR` must already exist and
be writable; the suite verifies the main home, the allowlist workspace and
the security fixtures are created directly beneath it before continuing.
Cleanup-check subshells create fixtures beneath separate `TMPDIR` roots
inside the main home, including roots whose names contain spaces, and verify
each fixture is removed on exit.

After the main home is created, an `EXIT` trap removes the directories that
have been created and stops background MCP servers. The graph fallback also
removes its home immediately after a successful import; the exit trap covers
ordinary failure paths. A termination that prevents the trap from running, a
failure before its installation, or a removal error can leave directories
behind. Inspect leftovers before removing them, and do not remove the parent
`TMPDIR` or unrelated files.

Run `node --test scripts/e2e-temp.test.mjs` to check the documented variable
and prefix inventory against the script and exercise temporary-directory cleanup.
Run `node --test scripts/e2e-home-selection.test.mjs` for the focused
`LOCUS_HOME` temporary-root selection checks,
`node --test scripts/e2e-home-tmpdir-containment.test.mjs` for `LOCUS_HOME`
containment beneath the controlled `TMPDIR` and fail-closed selection,
`node --test scripts/e2e-home-spaced-tmpdir.test.mjs` for `LOCUS_HOME`
allocation beneath a space-containing `TMPDIR`, and
`node --test scripts/e2e-graph-home-selection.test.mjs` for the `GRAPH_HOME`
equivalent,
`node --test scripts/e2e-security-workspace-spaced-tmpdir.test.mjs` for the
`SECURITY_WS` space-containing `TMPDIR` checks, and
`node --test scripts/e2e-workspace-spaced-tmpdir.test.mjs` for the `WS_DIR`
equivalent.
Run `scripts/tests/e2e-tmpdir.sh` to exercise the full suite under a custom
`TMPDIR`, including an injected failure after workspace creation.
See the [security directories](e2e-security-directories.md) for the
security probes and the [E2E test map](e2e-test-map.md) for the full suite.
