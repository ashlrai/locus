# Fleet preflight — before agent dispatch

Exact steps ashlr-hub (or any fleet orchestrator) must run **before** dispatching a mutating agent job. Fail closed.

Contract: [`docs/hub-integration.md`](../../docs/hub-integration.md)  
Drop-in: [`locus.ts`](./locus.ts) → `locusFleetGate()` / `evaluateFleetGate()` / `assertLocusPreMutate()` / `locusWatchOnce()` / `locusVerifySession()`  
Schema: [`schema/hub-gate.schema.json`](../../schema/hub-gate.schema.json)

---

## Goal

Wrong-account action must be **mechanically impossible** for hub-spawned agents:

1. Identity plane is ready (`status=ready`, healthy pin oneline).
2. Catalog is locus-first (`required_servers` = locus + phantom only).
3. Job pin is ephemeral when concurrent (`ci mint` / `withLocusSession`) — no shared `active.json` races or ambient credential inheritance.
4. Secrets never enter hub logs or model context.

---

## Preflight checklist (exact order)

### 0. Environment

```bash
export PATH="${PATH}:$HOME/.cargo/bin"
export LOCUS_HOME="${JOB_LOCUS_HOME:-$HOME/.locus}"
export LOCUS_NOTIFY=0
export LOCUS_QUIET=1
# Optional: LOCUS_BIN if not on PATH
```

| Variable | Required | Notes |
|----------|----------|--------|
| `LOCUS_HOME` | yes (default `~/.locus`) | Job-local path preferred in CI |
| `LOCUS_SESSION_ID` | CI children | From `locus ci mint` / `withLocusSession` |
| `LOCUS_ENFORCE` | opt-in at spawn sites | Env wins over firm config. `1`/`enforce` fail closed; `warn` log-only; unset → `locus.enforce` then `locus.firm` then off |
| `LOCUS_CI_BINDING` / `LOCUS_BINDING` | CI mint overlay | When set, `runWithLocusSessionIfConfigured` mints ephemeral pin (hub `runSwarm` / `runTask`) |
| `LOCUS_NOTIFY` | recommended `0` | Quiet hub children |
| `PATH` | `locus` + `locus-mcp` | Or set `LOCUS_BIN` |

**Firm config** (`~/.ashlr/config.json`, hub #254 / #258):

```json
{ "locus": { "firm": true } }
// or explicit: { "locus": { "enforce": "enforce" } }
```

Resolution: env `LOCUS_ENFORCE` (if set) → `locus.enforce` → `locus.firm === true` → `off`. Never always-on; firm defaults false.

### 1. CLI present

```ts
import { locusAvailable } from "./locus";
if (!locusAvailable()) {
  // BLOCK — install locus-cli; do not dispatch
}
```

```bash
command -v locus >/dev/null || exit 2
```

### 2. Single readiness probe — `locusFleetGate()` / `assertLocusPreMutate()`

**Preferred hub entrypoint** when you always want a CLI probe (wraps `locus agent report --json` + contract checks):

```ts
import {
  locusFleetGate,
  ensureLocusReady,
  withLocusSession,
  applyLocusPreMutateGate,
  formatPreMutateBlockers,
} from "./locus";

const gate = locusFleetGate();
// gate: { allowDispatch, blockers[], report }
if (!gate.allowDispatch) {
  // Surface gate.blockers to human / job UI; DO NOT dispatch
  throw new Error(`locus fleet gate blocked: ${gate.blockers.join("; ")}`);
}
// Safe to dispatch under gate.report pin
```

**Shared spawn sites** (fleet/run engines) should use the opt-in pre-mutate wrapper so unset enforce mode does not break monorepo CI:

```ts
const pre = applyLocusPreMutateGate(); // mode from LOCUS_ENFORCE / locus.enforce / locus.firm
if (!pre.allow) {
  throw new Error(formatPreMutateBlockers(pre) || "locus pre-mutate blocked");
}
```

| Mode source | Result |
|-------------|--------|
| env unset **and** no `locus.enforce` / firm false / off | `allow: true`, no CLI probe |
| `warn` / `log` (env or config.enforce) | probes fleet gate; `allow: true` + `shouldWarn` when blocked |
| `1` / `enforce` / … (env or config) **or** `locus.firm: true` | probes fleet gate; `allow: false` when blocked |

Hub call sites (opt-in): pre-mutate on `spawnEngine` / `runSwarmInternal` / `runApiModelSandboxed`; CI session mint on `runSwarm` / `runTask` via `runWithLocusSessionIfConfigured`.

Schema of the fleet-gate return value: [`schema/hub-gate.schema.json`](../../schema/hub-gate.schema.json).

| Field | Rule |
|-------|------|
| `allowDispatch` | Must be `true` to dispatch mutating work |
| `blockers` | Empty iff allow; log these, never secrets |
| `report` | Full agent report or `null` |

Equivalent pure path (when you already have report JSON):

```ts
import {
  parseAgentReportJson,
  evaluateFleetGate,
  decidePreMutateGate,
  resolveLocusEnforceMode,
} from "./locus";

const report = parseAgentReportJson(stdout);
const { allowDispatch, blockers } = evaluateFleetGate(report);
const decision = decidePreMutateGate(
  { allowDispatch, blockers },
  resolveLocusEnforceMode(process.env),
);
```

### 3. What the gate checks (fail closed)

| Check | Blocker example |
|-------|-----------------|
| CLI missing | `locus CLI not found on PATH` |
| Report missing / bad JSON | `no agent report` / parse error |
| `status === "unsafe"` | `status=unsafe` |
| `status !== "ready"` | `status=protected (not ready)` |
| `ready !== true` | `ready=false` |
| Unhealthy oneline | `pin unhealthy: unpinned (unpinned)` |
| Bad seal / expired / frozen pin | `pin.seal_ok=false` etc. |
| `required_servers` lacks locus or phantom | `required_servers missing …` |
| `mcp_command !== "locus-mcp"` | `mcp_command must be locus-mcp` |

`status_oneline` healthy tokens:

| Token | Dispatch? |
|-------|-----------|
| `alias:tenant` | yes (if status ready) |
| `unpinned` | **no** |
| `require_pin` | **no** |
| `frozen` | **no** |
| `invalid` | **no** |

### 4. MCP catalog — locus-first

Before spawning the gateway / agent MCP client:

1. Register only `required_servers` from the report (`locus` + `phantom`).
2. Do **not** fan in ambient supabase / vercel / github MCP with personal tokens.
3. Ensure project MCP config includes locus:

```ts
import { registerLocusInMcpConfig, locusMcpServerSpecs } from "./locus";

// Project .mcp.json (Claude-style)
registerLocusInMcpConfig(".mcp.json", {
  locusHome: process.env.LOCUS_HOME,
  client: "ashlr-hub",
  // sessionId: mint.session_id,  // when using ci mint
});

// Or in-memory specs for the gateway:
const servers = locusMcpServerSpecs(process.env.LOCUS_HOME);
// merge phantom from hub ecosystem probe
```

See [mcp-gateway-snippet.md](./mcp-gateway-snippet.md).

### 5. Pin isolation for concurrent jobs

**Interactive / long-lived agent** (human shell pin or existing `LOCUS_SESSION_ID`):

```ts
ensureLocusReady(); // throws LocusNotReadyError
// or: const gate = locusFleetGate(); if (!gate.allowDispatch) …
```

**Hub / CI job** (preferred — does not mutate human `active.json`):

```ts
await withLocusSession("acme", async ({ env, sessionId }) => {
  // env is scrubbed: runtime basics + caller-explicit + validateMintEnv(ci mint)
  // — no ambient GITHUB_TOKEN / AWS_PROFILE / etc. from the parent process
  const gate = locusFleetGate(env);
  if (!gate.allowDispatch) {
    throw new Error(gate.blockers.join("; "));
  }
  // spawn agent + locus-mcp with `env` (includes LOCUS_SESSION_ID)
});
```

```bash
mint="$(locus ci mint -b acme --json)"
export LOCUS_SESSION_ID="$(printf '%s' "$mint" | jq -r .session_id)"
locus agent report --json   # env_session_id set; gate against this pin
```

Security helpers (pure; also used inside mint):

| Helper | Role |
|--------|------|
| `scrubbedChildEnv(parent, explicit?)` | Drop ambient credentials; keep `PATH`/`HOME`/`LC_*` + explicit |
| `validateMintEnv(raw)` | Allowlist identity/scope keys from mint JSON; reject locators/secrets |

### 6. Dispatch

Only after `allowDispatch === true`:

1. Spawn agent with MCP catalog = locus (+ phantom).
2. Forward `LOCUS_HOME` + `LOCUS_SESSION_ID` into children.
3. Agents may call `locus_whoami` / `locus_status` / `locus_request_pin` — **never** re-pin from the model.
4. Destructive tools may hit `require_approval`. Only a closed, independently authenticated external authorization envelope may release provider execution. Local CLI/dashboard labels are advisory; do not retry or redispatch after recording one. This release has no external verifier, so keep the job blocked.

### 7. On block — human remediation

| Blocker class | Human action |
|---------------|--------------|
| CLI missing | Install `locus-cli` / fix PATH |
| unpinned / protected | `locus enter <alias>` or workspace auto-pin |
| MCP unwired | `locus agent setup --apply --client all` or `registerLocusInMcpConfig` |
| unsafe / invalid seal | `locus doctor`; re-init only if seal key lost (audit) |
| required_servers contract | Upgrade locus; do not soft-allow missing phantom |
| `require_approval` pending | Keep provider execution blocked pending a closed external authorization envelope; never convert local labels into retry authority |

Surface `report.next_steps[0]` when present.

---

## Bash one-liner gate (scripts / CI)

```bash
set -euo pipefail
export LOCUS_HOME="${LOCUS_HOME:-$HOME/.locus}"

report="$(locus agent report --json)" || true
printf '%s' "$report" | jq -e '
  .status == "ready"
  and .ready == true
  and (.status_oneline | test("^(unpinned|require_pin|frozen|invalid)$") | not)
  and (.required_servers | index("locus") != null)
  and (.required_servers | index("phantom") != null)
  and .mcp_command == "locus-mcp"
' >/dev/null
```

Full composition smoke: [`scripts/hub-integration-test.sh`](../../scripts/hub-integration-test.sh).

---

## Optional: session heartbeat (soft)

For continuous fleet whoami / doctor annotation (not a substitute for the hard
pre-mutate gate), shell the verification plane:

```ts
import {
  locusWatchOnce,
  locusVerifySession,
  locusSoftWatchHeartbeat,
  parseWatchHeartbeat,
} from "./locus";

// Compact tick — aliases/verdicts only
const tick = locusWatchOnce();
// Full pack when doctor/whoami objects are needed
const pack = locusVerifySession();
// Soft-only under LOCUS_ENFORCE=warn (null when mode≠warn; never hard-blocks alone)
const soft = locusSoftWatchHeartbeat();
```

```bash
locus watch --once --json
locus verify session --json
```

Do **not** treat a soft watch tick as the sole hard gate — keep
`applyLocusPreMutateGate` / `locusFleetGate` as the mutate fence. Shapes:
[docs/verification-plane.md](../../docs/verification-plane.md).

---

## What hub must never do at preflight

| Forbidden | Why |
|-----------|-----|
| Soft-allow `status=unsafe` or unhealthy oneline | Wrong-account risk |
| Soft-allow unknown `LOCUS_ENFORCE` tokens | Typo must fail closed to `enforce` |
| Parse resolved secrets from locus/phantom stdout | Secret opacity |
| Inherit full parent `process.env` into mint children | Ambient credential leak |
| Register ambient provider MCPs alongside locus for same accounts | Bypasses pin + freeze |
| Let the model call `locus pin` | Agents request only |
| Share one `active.json` pin across parallel mutate jobs | Race / cross-tenant |
| Hard-block solely on soft watch under `LOCUS_ENFORCE=warn` | Soft path is annotation-only |

---

## Related

| Doc | Topic |
|-----|--------|
| [locus.ts](./locus.ts) | `locusFleetGate`, `assertLocusPreMutate`, `locusWatchOnce`, `locusVerifySession`, pure parsers |
| [mcp-gateway-snippet.md](./mcp-gateway-snippet.md) | REQUIRED_SERVERS + discovery |
| [doctor-check.md](./doctor-check.md) | `checkLocus` for ashlr doctor |
| [docs/hub-integration.md](../../docs/hub-integration.md) | Full CLI contract |
| [schema/hub-gate.schema.json](../../schema/hub-gate.schema.json) | Gate response schema |
| [schema/agent-report.schema.json](../../schema/agent-report.schema.json) | Nested report |
