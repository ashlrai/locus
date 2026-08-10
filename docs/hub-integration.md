# Ashlr Hub ↔ Locus integration contract

Machine contract for **ashlr-hub** (and similar orchestrators) to shell out to Locus cleanly.

**Sibling:** [Phantom](https://phm.dev) answers *can this secret enter the model?*  
**Locus** answers *as whom, against which tenant, right now?*

| Artifact | Path |
|----------|------|
| Agent report schema | [`schema/agent-report.schema.json`](../schema/agent-report.schema.json) |
| Doctor schema | [`schema/doctor.schema.json`](../schema/doctor.schema.json) |
| Fleet gate schema | [`schema/hub-gate.schema.json`](../schema/hub-gate.schema.json) |
| Hub drop-in + TS types | [`integrations/ashlr-hub/locus.ts`](../integrations/ashlr-hub/locus.ts) |
| Fleet preflight | [`integrations/ashlr-hub/fleet-preflight.md`](../integrations/ashlr-hub/fleet-preflight.md) |
| MCP gateway patch | [`integrations/ashlr-hub/mcp-gateway-snippet.md`](../integrations/ashlr-hub/mcp-gateway-snippet.md) |
| Doctor `checkLocus` | [`integrations/ashlr-hub/doctor-check.md`](../integrations/ashlr-hub/doctor-check.md) |
| Northstar | [`GOALS.md`](../GOALS.md) · `locus goal status` |
| Smoke | [`scripts/hub-smoke.sh`](../scripts/hub-smoke.sh), [`scripts/hub-integration-test.sh`](../scripts/hub-integration-test.sh) |

---

## Invariants for hub agents

1. **Shell out; do not reimplement seal/pin.** Use CLI JSON or MCP control tools only.
2. **NEVER parse secrets or credential locators from locus output.** Results expose aliases, tenants, credential presence/source metadata, scopes, and issue codes only.
3. **Register one MCP server named `locus`**, not raw `supabase` / `vercel` / `github` MCPs with ambient credentials. Provider tools come from the pin via `locus-mcp`.
4. **REQUIRED_SERVERS:** `locus` + `phantom` (also emitted on every agent report as `required_servers`).
5. **Agents cannot pin.** Only humans (or audited CI) run `locus pin` / `locus enter`. Agents may `locus_request_pin`; hub surfaces that to a human.
6. **Fail closed.** `status=unsafe`, invalid seal, or `status_oneline` in `{unpinned,frozen,invalid}` for mutating work → do not proceed.

---

## Environment

| Variable | Role |
|----------|------|
| `LOCUS_HOME` | Store root (default `~/.locus`). Use a dedicated path in tests/CI. |
| `LOCUS_SESSION_ID` | Optional. When set (e.g. after `locus ci mint`), locus resolves that sealed session instead of `sessions/active.json`. Echoed on agent report as `env_session_id` when present. |
| `LOCUS_ENFORCE` | Opt-in pre-mutate / CI session gate for hub spawn sites. Wins over `config.locus.enforce` / `config.locus.firm` when set. See [Pre-mutate enforce](#pre-mutate-enforce-locus_enforce--configlocus) below. |
| `LOCUS_CI_BINDING` / `LOCUS_BINDING` | Optional. When set, `runWithLocusSessionIfConfigured` mints an ephemeral pin (`ci mint`) for fleet/single-task paths instead of ambient `active.json`. |
| `LOCUS_BIN` | Optional override for the `locus` binary path. |
| `LOCUS_NOTIFY` / `LOCUS_QUIET` | Prefer `0` / `1` in hub children (drop-in sets these on mint handles). |

```bash
export LOCUS_HOME="${LOCUS_HOME:-$HOME/.locus}"
# CI / ephemeral child only:
# export LOCUS_SESSION_ID="$(jq -r .session_id < mint.json)"
# Production hub mutate paths (opt-in env override):
# export LOCUS_ENFORCE=1
# export LOCUS_CI_BINDING=acme
```

Hub children that inherit a minted session **must** pass `LOCUS_SESSION_ID` (and usually `LOCUS_HOME`) into the process env. Do not invent session ids.

### Pre-mutate enforce (`LOCUS_ENFORCE` + `config.locus`)

Shared hub spawn sites should call **`assertLocusPreMutate()`** / **`applyLocusPreMutateGate()`** (not bare `ensureLocusReady`) so monorepo CI without a pin stays green by default.

**Mode resolution** (`resolveLocusEnforceMode` — first match wins; never always-on):

1. Env `LOCUS_ENFORCE` when the key is present (including empty / `off` → override firm config)
2. `~/.ashlr/config.json` → `locus.enforce` when set explicitly (`off`|`warn`|`enforce`)
3. `locus.firm === true` → `enforce` (production fleet profile; hub #258)
4. `off` (monorepo-safe default — firm defaults false)

| Token (env or config.enforce) | Mode | Behavior |
|-------------------------------|------|----------|
| unset / absent config / `0` / `false` / `no` / `off` / `""` | `off` | Allow without CLI probe (monorepo-safe default) |
| `warn` / `log` | `warn` | Shell `locusFleetGate`; log blockers; still allow |
| `1` / `true` / `yes` / `enforce` / `block` | `enforce` | Shell fleet gate; **deny** when blocked |
| any other non-empty value | `enforce` | Fail closed (typo ≠ soft-allow) |

Env always beats firm/enforce config (including `LOCUS_ENFORCE=off`). Explicit `locus.enforce` beats `locus.firm`. Do not flip monorepo defaults to firm.

**Firm profile** (production fleets — opt-in; no shell export required):

```json
// ~/.ashlr/config.json
{ "locus": { "firm": true } }
// # ashlr config set locus.firm true
```

Explicit enforce mode (overrides firm when set): `{ "locus": { "enforce": "enforce" } }`.  
Soft roll-out: `{ "locus": { "enforce": "warn" } }`.  
Local override without editing config: `LOCUS_ENFORCE=off`.

```ts
import {
  assertLocusPreMutate,
  applyLocusPreMutateGate,
  formatPreMutateBlockers,
  decidePreMutateGate,
  resolveLocusEnforceMode,
  extractLocusConfigEnforce,
  extractLocusConfigFirm,
  parseLocusEnforceToken,
} from "../integrations/ashlr-hub/locus";

// Preferred at spawn sites: logs warn/block to stderr
// (loads ~/.ashlr locus.enforce / locus.firm when LOCUS_ENFORCE is unset)
const decision = applyLocusPreMutateGate(); // or assertLocusPreMutate()
if (!decision.allow) {
  throw new Error(formatPreMutateBlockers(decision) || "locus pre-mutate blocked");
}
// Pure path when you already have a fleet-gate result + explicit config:
// decidePreMutateGate(gate, resolveLocusEnforceMode(env, { locus: { firm: true } }))
// Pass null as config to disable firm FS read in tests: assertLocusPreMutate(env, null)
```

**Hub production call sites** (opt-in only — never always-on):

| Call site | Gate |
|-----------|------|
| `spawnEngine` / `runSwarmInternal` / `runApiModelSandboxed` | `applyLocusPreMutateGate` (pre-mutate) |
| `runSwarm` / `runTask` | `runWithLocusSessionIfConfigured` (CI session mint overlay; hub #252) |

### Scrubbed mint env (`withLocusSession`)

`withLocusSession` / `locusCiMint` never forward ambient parent credentials into the child handle:

- **`scrubbedChildEnv(parent, explicit?)`** — keep only runtime basics (`PATH`, `HOME`, `TMPDIR`, `LC_*`, …) plus caller-explicit keys.
- **`validateMintEnv(mint.env)`** — allowlist identity/scope keys from `ci mint --json` (`LOCUS_SESSION_ID`, `LOCUS_*_PROJECT_REF`, `GH_CONFIG_DIR`, …). Rejects secret-shaped values and `phm:` / `env:` / `test:` locators.
- Handle env = scrubbed parent + validated mint map + `LOCUS_HOME` / `LOCUS_SESSION_ID` / quiet flags.

Do not merge `process.env` wholesale into job children.

---

## CLI contracts (shell these out)

Global flag: `--json` is available on most commands and is **required** for hub machine consumers.

### Exit codes

| Command | Exit 0 | Exit 1 | Exit 2 |
|---------|--------|--------|--------|
| `locus agent report` / `locus agent doctor` | `status=ready` | `status=protected` | `status=unsafe` |
| `locus doctor` | `verdict=SAFE` | `verdict=WARN` | `verdict=UNSAFE` |

---

### 1. `locus agent report --json`  ★ preferred hub entrypoint

Single snapshot: pin + doctor + MCP registration probe + next commands.  
Schema: [`schema/agent-report.schema.json`](../schema/agent-report.schema.json).

```bash
locus agent report --json
# also: locus agent report --json  (local flag) or locus --json agent report
# exit: 0 ready | 1 protected | 2 unsafe
```

**Stable top-level keys** (`AGENT_REPORT_JSON_KEYS`):

| Key | Type | Notes |
|-----|------|--------|
| `version` | string | CLI/crate version |
| `ready` | bool | `true` only when `status=ready` |
| `status` | `ready` \| `protected` \| `unsafe` | Hub gate |
| `pin` | object \| omitted | Active pin slice when present |
| `mcp_registered` | `{claude,cursor,codex}` | Whether server name `locus` is wired |
| `doctor` | object | Full doctor report (see below) |
| `commands` | object | Suggested next human commands |
| `exit_code` | 0 \| 1 \| 2 | Same as process exit |
| `status_oneline` | string | Same as `locus status --oneline` |
| `home` | path | Effective `LOCUS_HOME` |
| `env_session_id` | string? | Present when `LOCUS_SESSION_ID` was set |
| `required_servers` | `["locus","phantom"]` | **REQUIRED_SERVERS** |
| `mcp_command` | `"locus-mcp"` | Multiplexor binary |
| `findings` | string[] | Agent-plane gaps (optional empty) |
| `next_steps` | string[] | Human next actions |

**`status_oneline` tokens** (match `locus status --oneline`):

- `unpinned`
- `alias:tenant` (healthy pin)
- `frozen`
- `invalid`

**`pin` object** (when present): `alias`, `tenant`, `binding_id`, `expires_at`, `seal_ok`, `expired`, optional `principal` / `client`.

**`commands`**: at least `enter`, `whoami`; optional `pin`, `doctor`, `setup`.

**Status ladder:**

| status | Meaning | Hub policy |
|--------|---------|------------|
| `ready` | Seal ok, valid pin, bindings, MCP registered | OK to act under pin |
| `protected` | Control plane incomplete (no pin / no MCP / no bindings) | Soft block — setup or pin first |
| `unsafe` | Seal broken / doctor UNSAFE / invalid pin | Hard block |

Also available: `locus agent doctor` (same report, human-first default; honors `--json`).

---

### 2. `locus doctor --json`

Full mission-control pane (also nested under agent report as `.doctor`).  
Schema: [`schema/doctor.schema.json`](../schema/doctor.schema.json).

```bash
locus doctor --json
# exit: 0 SAFE | 1 WARN | 2 UNSAFE
```

Stable keys:  
`version`, `home`, `seal_ok`, `bindings`, `runtime`, `approvals`, `pending_approvals`, `dual_control_waiting`, `phantom_on_path`, `unresolved_phm`, `autopin`, `workspace`, `audit`, `findings`, `issues`, `verdict`, `ok`.

Optional when pinned: `pinned`, `pin`, `pin_seal_ok`.

`unresolved_phm` contains structured `{ provider, source, code }` issues. Locator names and values are absent.

---

### 3. `locus whoami --json`

Active pin identity. **Errors if unpinned** (use `status` / `agent report` for unpinned-safe checks).

```bash
locus whoami --json
```

Fields: `session_id`, `binding_alias`, `binding_id`, `tenant`, `principal?`, `providers[]`, `expires_at`, `worker_home`, `seal_ok`, `frozen`, `frozen_reason?`, `mode`, `namespaces?`.

`providers[].credential` contains only `{ present, source }`; locator names and values are absent.

---

### 4. `locus status --oneline` / `locus status --json`

```bash
locus status --oneline
# unpinned | alias:tenant | frozen | invalid

locus status --json
# unpinned: { "pinned": false }
# pinned:   { "pinned": true, "binding", "tenant", "session_id", "seal_ok",
#             "frozen", "frozen_reason", "expired", "mode", "namespaces" }
```

---

## MCP registration (hub clients)

**Do this — single multiplexor named `locus`:**

```json
{
  "mcpServers": {
    "locus": {
      "command": "locus-mcp",
      "args": [],
      "env": {
        "LOCUS_HOME": "${LOCUS_HOME}"
      }
    },
    "phantom": {
      "command": "phantom-mcp",
      "args": []
    }
  }
}
```

Human / agent setup:

```bash
locus agent setup --client all --apply
# or: locus setup --client claude
```

`locus agent setup` injects `LOCUS_AUTO_PIN=cwd` + `LOCUS_CLIENT=<client>` into MCP env. It **never** sets `LOCUS_NOTIFY` (desktop banners stay opt-in).

**Do not do this:**

- Register separate MCP servers for Supabase / Vercel / GitHub with long-lived env tokens that bypass the pin.
- Point agents at multiple account-specific MCPs and hope the model picks the right one.
- Log or forward resolved secrets from any child into hub memory / prompts.

Until a human pins, `locus-mcp` exposes **control tools only** (`locus_whoami`, `locus_status`, `locus_list_bindings`, `locus_request_pin`, …). No ambient personal fallthrough.

### REQUIRED_SERVERS recommendation

```ts
export const REQUIRED_SERVERS = ["locus", "phantom"] as const;
// Also: (await locusAgentReport()).required_servers
```

| Server | Why |
|--------|-----|
| `locus` | Identity plane + pinned provider tools only |
| `phantom` | Secret vault; private credential references resolve outside the model |

---

## Suggested hub wiring sequence

```bash
export LOCUS_HOME="${JOB_LOCUS_HOME:-$HOME/.locus}"

# Preferred single probe
set +e
report="$(locus agent report --json)"
code=$?
set -e

status="$(printf '%s' "$report" | jq -r .status)"
oneline="$(printf '%s' "$report" | jq -r .status_oneline)"
ready="$(printf '%s' "$report" | jq -r .ready)"

# Mutating work gate (example)
# [[ "$status" == "ready" && "$oneline" != unpinned && "$oneline" != frozen && "$oneline" != invalid ]]

# MCP catalog = only required_servers from report (locus + phantom)
# Never scrape secrets from stdout of locus, locus-mcp, or phantom list tools
```

CI ephemeral pin (optional):

```bash
mint="$(locus ci mint -b acme --json)"
export LOCUS_SESSION_ID="$(printf '%s' "$mint" | jq -r .session_id)"
locus agent report --json   # resolves via LOCUS_SESSION_ID; env_session_id set
```

---

## What hub must never do

| Forbidden | Why |
|-----------|-----|
| Parse/display resolved API keys from locus stdout | Secrets must not enter model/hub context |
| Call raw provider MCPs alongside locus for the same accounts | Bypasses pin / scope freeze |
| Let the model invent `project_ref` / `team_id` | Scope freeze is load-bearing |
| Agent-initiated `locus pin` without human/CI policy | Agents may only `locus_request_pin` |
| Soft-allow on `status=unsafe` or invalid seal | Fail closed |

---

## Fleet gate (hub pre-dispatch)

```ts
import {
  locusFleetGate,
  applyLocusPreMutateGate,
  formatPreMutateBlockers,
  registerLocusInMcpConfig,
  withLocusSession,
} from "../integrations/ashlr-hub/locus";

// Strict preflight (always probes CLI)
const gate = locusFleetGate(); // { allowDispatch, blockers[], report }
if (!gate.allowDispatch) {
  // do not dispatch mutating agents
}

// Opt-in spawn-site gate (default off until LOCUS_ENFORCE, config.locus.enforce, or locus.firm)
const pre = applyLocusPreMutateGate();
if (!pre.allow) throw new Error(formatPreMutateBlockers(pre));

registerLocusInMcpConfig(".mcp.json"); // merge locus into MCP JSON

// Ephemeral job pin — scrubbed child env, no active.json mutation
await withLocusSession("acme", async ({ env }) => {
  const g = locusFleetGate(env);
  if (!g.allowDispatch) throw new Error(g.blockers.join("; "));
});
```

Schema: [`schema/hub-gate.schema.json`](../schema/hub-gate.schema.json).  
Exact steps: [`integrations/ashlr-hub/fleet-preflight.md`](../integrations/ashlr-hub/fleet-preflight.md).

---

## Validation

```bash
# From repo root (jq + node required for composition smoke)
export PATH="$HOME/.cargo/bin:$PATH"
./scripts/hub-smoke.sh
./scripts/hub-integration-test.sh

# Manual:
export LOCUS_HOME=/tmp/locus-hub-smoke
locus init --with-samples
locus pin personal
locus agent report --json | jq -e '
  .ready != null
  and (.status | IN("ready","protected","unsafe"))
  and .required_servers == ["locus","phantom"]
  and .mcp_command == "locus-mcp"
  and .status_oneline != null
'
locus doctor --json        | jq -e '.verdict and (.ok | type == "boolean")'
locus whoami --json        | jq -e '.binding_alias and .session_id'
locus status --oneline
```

---

## Related docs

| Doc | Topic |
|-----|--------|
| [mcp.md](./mcp.md) | Client wiring for locus-mcp |
| [architecture.md](./architecture.md) | Planes diagram |
| [policy.md](./policy.md) | allow / deny / require_approval |
| [firm-mode.md](./firm-mode.md) | Multi-client agency ops |
| [../DESIGN.md](../DESIGN.md) | Full threat model |
