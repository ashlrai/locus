# Hub smoke temporary resources

[`scripts/hub-smoke.sh`](../scripts/hub-smoke.sh) creates these three paths
under `TMPDIR`, or `/tmp` when `TMPDIR` is unset or empty. `mktemp` replaces
the `XXXXXX` suffix with a unique value.

| Variable | Type | Template | Purpose |
| --- | --- | --- | --- |
| `SMOKE_HOME` | Directory | `locus-hub-smoke.XXXXXX` | Exported as `LOCUS_HOME` to isolate sample bindings, the pin, and other Locus state. |
| `SMOKE_PROJ` | Directory | `locus-hub-proj.XXXXXX` | Working project where the script writes `.mcp.json` for agent report checks. |
| `SMOKE_ERR` | File | `locus-hub-smoke-err.XXXXXX` | Captures init and pin stderr for retry diagnostics; truncated on each attempt. |

An `EXIT` trap removes the assigned paths, including after ordinary setup
failures. An uncatchable kill can prevent cleanup, and removal can fail. See
the [temporary resource guide](hub-smoke-temp-resources.md) and
[resource map](hub-smoke-resource-map.md) for details and other resources.
