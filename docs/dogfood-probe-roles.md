# Dogfood probe directory roles

[`scripts/dogfood.sh`](../scripts/dogfood.sh) allocates three independent
temporary directory roles under `${TMPDIR:-/tmp}`. An unset or empty `TMPDIR`
uses `/tmp`; otherwise the parent must already exist and be writable. Each
`XXXXXX` suffix is supplied by `mktemp -d`. These directories isolate test
state from each other; they are not three names for the same Locus home.

| Template | Role | Probe and readiness effect |
| --- | --- | --- |
| `locus-dogfood.XXXXXX` | Main isolated `LOCUS_HOME`, with a `project` working directory and, by default, `pack.json`. The script creates a synthetic GitHub binding here. | The main flow runs quickstart, agent setup and report, doctor, forensics export, session verification, and goal status. Report, doctor and session evidence must agree at the final readiness gate; the pack is checked for JSON shape and possible secrets or credential locators. |
| `locus-sandbox-dogfood.XXXXXX` | Sandbox probe root, containing a separate `home`, project, local MCP fixture and captured MCP stdout/stderr. | Step 1b pins a synthetic binding and calls the fixture through the upstream worker with secret resolution disabled. On macOS it requires the `sandbox-exec` marker and backend; on other platforms it checks for a fail-closed unsupported-backend refusal, but still cannot declare `DOGFOOD READY`. A failed attestation blocks readiness. |
| `locus-mt-dogfood.XXXXXX` | Optional multi-tenant probe root, with its own `home`, working directory and local MCP server artifacts. | With `DOGFOOD_MT=1`, step 8b mints a grant, checks the roster and HTTP responses (tenantless 401, granted 200), revokes it, and checks for 401 and no live roster entry. It must pass when enabled; by default it does not run. |

The sandbox and multi-tenant probes run in subshells with their own homes and
do not replace the main pin. The multi-tenant probe also changes into its own
root so a caller's workspace allowlist cannot affect its synthetic binding.
The fixture makes no credential lookup or network request; the multi-tenant
HTTP checks use a local server.

By default the main flow uses the isolated home and applies agent setup there.
`DOGFOOD_USE_REAL_HOME=1` instead avoids allocating the main directory, uses
`DOGFOOD_HOME` if set or the default Locus home otherwise, and defaults setup to
dry-run; the sandbox and enabled multi-tenant probes remain separate. An
explicit `DOGFOOD_APPLY` changes the setup choice. The default forensic pack is
inside the temporary main home and is removed with it; `DOGFOOD_PACK` can
choose another path, which survives only if it is outside a removed directory.
In real-home mode the default pack path is `/tmp/pack.json`.

The main exit trap removes its temporary home, and each probe's subshell exit
trap removes its own root; the multi-tenant trap also stops and waits for its
server. Traps cover ordinary exits and trappable failures, not uncatchable
termination or failed removal. Inspect leftovers before deleting them. Other
resources have separate lifecycles: the required `hub-smoke.sh` helper creates
its own temporary home, project and error file (see the [Hub resource map](hub-test-temp-comparison.md));
the optional client probe inspects client installations. A diagnostic
`DOGFOOD_SKIP_HUB_SMOKE=1` cannot reach readiness. Build artifacts in `target/`
are not removed by these directory traps.

For the concise prefix inventory, see the [temporary-directory reference](dogfood-temp-prefix-reference.md).
Run `node --test scripts/dogfood.test.mjs` to check the inventory, probe roles,
and existing cleanup paths.
