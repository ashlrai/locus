# Hub smoke temporary resource roles

[`scripts/hub-smoke.sh`](../scripts/hub-smoke.sh) checks local CLI contracts
using three temporary paths. Each allocation uses a unique `mktemp` suffix
under a nonempty `TMPDIR`, or `/tmp` otherwise.

| Resource | Creation site | Role in the smoke test |
| --- | --- | --- |
| `SMOKE_HOME` | [First `mktemp -d`](../scripts/hub-smoke.sh#L28) | Becomes `LOCUS_HOME`, isolating sample bindings, the pin and other Locus state from the user's real home. |
| `SMOKE_PROJ` | [Second `mktemp -d`](../scripts/hub-smoke.sh#L32) | Provides the working project in which the script writes `.mcp.json` for agent report checks. |
| `SMOKE_ERR` | [`mktemp` file allocation](../scripts/hub-smoke.sh#L36) | Captures init and pin stderr for retry and failure diagnostics; it is truncated on each attempt. |

The script installs an `EXIT` cleanup trap before allocating these paths.
Cleanup is best effort and cannot run after an uncatchable kill. Diagnostics
may be sensitive. For allocation failures, cleanup details and other resources,
see the [temporary path lifecycle](hub-smoke-temp-lifecycle.md) and
[resource map](hub-smoke-resource-map.md).
