# E2E temporary directory prefixes

[`scripts/e2e.sh`](../scripts/e2e.sh) assigns these `mktemp -d` templates to
script variables. Each `XXXXXX` is replaced by `mktemp` when it creates a
directory; the prefix includes the final dot.

| Variable | Prefix | Purpose |
| --- | --- | --- |
| `LOCUS_HOME` | `locus-e2e.` | Main isolated Locus home and suite fixtures. |
| `SECURITY_HOME` | `locus-security-e2e.` | Home for security probes and the separate cleanup-check subshell. |
| `WS_DIR` | `locus-ws.` | Workspace for binding allowlist checks. |
| `GRAPH_HOME` | `locus-graph-imp.` | Fresh home used only when graph import needs a fallback. |
| `SECURITY_WS` | `locus-security-ws.` | Workspace for adversarial security probes. |

The directories are created under a nonempty `TMPDIR`, or `/tmp` when it is
unset or empty. The security cleanup-check subshell sets its own `TMPDIR`
inside the main home. The suite's exit trap removes its directories; a
successful graph fallback also removes its home immediately. Abrupt
termination or removal errors can leave directories behind. Inspect leftovers
before removing them, and preserve the parent temporary directory and unrelated
files. See the [temporary resource reference](e2e-temp-resource-types.md) for
further details.
