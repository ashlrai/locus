# ashlr-hub ↔ Locus drop-in

Notes and types for wiring **ashlr-hub** (or any agent orchestrator) to Locus without forking this repo.

| Artifact | Path |
|----------|------|
| Full contract | [`docs/hub-integration.md`](../../docs/hub-integration.md) |
| TypeScript probe | [`locus.ts`](./locus.ts) — `locusFleetGate`, `assertLocusPreMutate`, `ensureLocusReady`, `withLocusSession` (scrubbed mint env), `locusVerifySession` / `locusWatchOnce` (fleet heartbeat), `registerLocusInMcpConfig`, pure parsers |
| Fleet preflight | [`fleet-preflight.md`](./fleet-preflight.md) — exact steps before agent dispatch |
| MCP gateway patch | [`mcp-gateway-snippet.md`](./mcp-gateway-snippet.md) |
| Doctor check | [`doctor-check.md`](./doctor-check.md) |
| Schemas | [`schema/agent-report.schema.json`](../../schema/agent-report.schema.json), [`schema/doctor.schema.json`](../../schema/doctor.schema.json), [`schema/hub-gate.schema.json`](../../schema/hub-gate.schema.json) |
| Northstar | [`GOALS.md`](../../GOALS.md) · `locus goal status` |

---

## What hub should do

1. Shell out to Locus CLI (or spawn `locus-mcp` stdio) — do not reimplement pin/seal.
2. Prefer **`locusFleetGate()`** (or `ensureLocusReady()`) before agent dispatch — see [fleet-preflight.md](./fleet-preflight.md).
3. At shared spawn sites use **`applyLocusPreMutateGate()`** with opt-in `LOCUS_ENFORCE=1|warn`, firm `config.locus.enforce`, or `config.locus.firm: true` (default off = monorepo-safe; env wins).
4. Register MCP servers from **`required_servers`** (`locus` + `phantom` only) — `registerLocusInMcpConfig` / [mcp-gateway-snippet.md](./mcp-gateway-snippet.md).
5. Use **`withLocusSession(binding, fn)`** for ephemeral job pins (`ci mint`; scrubbed child env + `validateMintEnv`; no `active.json` mutation).
6. For continuous session health, shell **`locusWatchOnce()`** / **`locusVerifySession()`** (or pure `parseWatchHeartbeat` / `parseSessionVerificationPack`) — soft annotation under `LOCUS_ENFORCE=warn` via `locusSoftWatchHeartbeat`; never a hard blocker alone.
7. Add **`checkLocus`** to ashlr doctor — see [doctor-check.md](./doctor-check.md).
8. **Never** parse or store secret values from locus/phantom output.

## What hub must not do

- Register raw Supabase / Vercel / GitHub MCP servers with ambient tokens for the same accounts Locus pins.
- Let agents re-pin or invent `project_ref` / `team_id`.
- Soft-allow when `status === "unsafe"` or seal is bad.

---

## REQUIRED_SERVERS

```ts
/** Ashlr agent safety pair — identity plane + secret plane. */
export const REQUIRED_SERVERS = ["locus", "phantom"] as const;
export type RequiredServer = (typeof REQUIRED_SERVERS)[number];
```

Also present on every report: `report.required_servers` and `report.mcp_command === "locus-mcp"`.

Example MCP config fragment:

```json
{
  "mcpServers": {
    "locus": {
      "command": "locus-mcp",
      "args": [],
      "env": { "LOCUS_HOME": "${LOCUS_HOME}" }
    },
    "phantom": {
      "command": "phantom-mcp",
      "args": []
    }
  }
}
```

Optional env for CI children: `LOCUS_SESSION_ID` (from `locus ci mint --json`).

Pre-mutate enforce at hub spawn sites (env `LOCUS_ENFORCE` wins over `~/.ashlr/config.json` → `locus.enforce` then `locus.firm`; hub #254 / #258):

| Mode (env / config) | Behavior |
|---------------------|----------|
| unset / `off` / `0` / absent config / `firm` false | No CLI probe; allow (default) |
| `warn` / `log` | Probe fleet gate; log blockers; allow |
| `1` / `true` / `enforce` / `firm: true` | Probe fleet gate; **block** when unhealthy |

Firm profile (production fleets only — never monorepo default):

```json
{ "locus": { "firm": true } }
```

---

## CLI cheat sheet

```bash
export LOCUS_HOME="${LOCUS_HOME:-$HOME/.locus}"

locus agent report --json   # ★ hub entrypoint (exit 0|1|2)
locus doctor --json         # full mission-control pane (SAFE|WARN|UNSAFE)
locus whoami --json         # requires pin
locus status --oneline      # unpinned | alias:tenant | frozen | invalid
locus status --json
locus verify session --json # doctor + whoami + safe_next pack (hub heartbeat)
locus watch --once --json   # compact NDJSON tick (kind=watch)
```

| Command | Exit 0 | Exit 1 | Exit 2 |
|---------|--------|--------|--------|
| `agent report` | ready | protected | unsafe |
| `doctor` | SAFE | WARN | UNSAFE |

---

## Example TypeScript types (hub-side)

Copy-paste friendly. Keep in sync with `schema/agent-report.schema.json`.

```ts
/** CLI exit codes for agent report / agent doctor. */
export type LocusAgentExitCode = 0 | 1 | 2;

export type AgentStatus = "ready" | "protected" | "unsafe";
export type DoctorVerdict = "SAFE" | "WARN" | "UNSAFE";

export interface DoctorPin {
  alias: string;
  tenant: string;
  binding_id: string;
  expires_at: string;
  seal_ok: boolean;
  principal?: string | null;
  client?: string | null;
  expired: boolean;
}

export interface McpRegistered {
  claude: boolean;
  cursor: boolean;
  codex: boolean;
}

export interface AgentCommands {
  enter: string;
  whoami: string;
  pin?: string | null;
  doctor?: string | null;
  setup?: string | null;
}

export interface ProviderView {
  provider: string;
  account: string;
  /** Agent-safe metadata; locator name and value are intentionally absent. */
  credential: { present: boolean; source: string };
  project_ref?: string | null;
  team_id?: string | null;
  account_id?: string | null;
  read_only?: boolean | null;
  orgs: string[];
}

/** Nested doctor report — full shape in schema/doctor.schema.json */
export interface DoctorReport {
  version: string;
  home: string;
  seal_ok: boolean;
  bindings: number;
  pinned?: string | null;
  pin?: DoctorPin | null;
  pin_seal_ok?: boolean | null;
  pending_approvals: number;
  dual_control_waiting: number;
  phantom_on_path: boolean;
  /** Safe resolution issues; locator names and provider stderr are absent. */
  unresolved_phm: Array<{ provider: string; source: string; code: string }>;
  verdict: DoctorVerdict;
  ok: boolean;
  findings: Array<{ severity: "warn" | "unsafe"; code: string; message: string }>;
  issues: string[];
  runtime: Record<string, unknown>;
  approvals: Record<string, unknown>;
  autopin: Record<string, unknown>;
  workspace: Record<string, unknown>;
  audit: Record<string, unknown>;
}

/** Output of `locus agent report --json`. */
export interface AgentReport {
  version: string;
  ready: boolean;
  status: AgentStatus;
  pin?: DoctorPin | null;
  mcp_registered: McpRegistered;
  doctor: DoctorReport;
  commands: AgentCommands;
  findings?: string[];
  next_steps?: string[];
  exit_code: LocusAgentExitCode;
  /** Same as `locus status --oneline` */
  status_oneline: string;
  home: string;
  env_session_id?: string | null;
  required_servers: string[]; // ["locus","phantom"]
  mcp_command: "locus-mcp" | string;
}

/** Minimal whoami shape from `locus whoami --json`. */
export interface WhoamiJson {
  session_id: string;
  binding_alias: string;
  binding_id: string;
  tenant: string;
  principal?: string | null;
  providers: ProviderView[];
  expires_at: string;
  worker_home: string;
  seal_ok: boolean;
  frozen: boolean;
  frozen_reason?: string | null;
  mode: string;
  namespaces?: string[];
}

export interface StatusPinnedJson {
  pinned: true;
  binding: string;
  tenant: string;
  session_id: string;
  seal_ok: boolean;
  frozen: boolean;
  frozen_reason?: string | null;
  expired: boolean;
  mode: string;
  namespaces: string[];
}

export interface StatusUnpinnedJson {
  pinned: false;
}

export type StatusJson = StatusPinnedJson | StatusUnpinnedJson;
```

### Drop-in helpers ([`locus.ts`](./locus.ts))

```ts
import {
  REQUIRED_SERVERS,
  locusAgentReport,
  locusFleetGate,
  ensureLocusReady,
  applyLocusPreMutateGate,
  formatPreMutateBlockers,
  withLocusSession,
  scrubbedChildEnv,
  validateMintEnv,
  parseStatusOneline,
  parseWatchHeartbeat,
  parseSessionVerificationPack,
  locusVerifySession,
  locusWatchOnce,
  locusSoftWatchHeartbeat,
  canMutate,
  evaluateFleetGate,
  registerLocusInMcpConfig,
  locusDoctorLine,
} from "./locus"; // copy of integrations/ashlr-hub/locus.ts

// Fleet pre-dispatch gate (preferred when always probing)
const gate = locusFleetGate(); // { allowDispatch, blockers[], report }
if (!gate.allowDispatch) throw new Error(gate.blockers.join("; "));

// Shared spawn sites — opt-in via LOCUS_ENFORCE / locus.enforce / locus.firm (default off)
const pre = applyLocusPreMutateGate();
if (!pre.allow) throw new Error(formatPreMutateBlockers(pre));

// Session heartbeat (aliases/verdicts only — never secrets)
const tick = locusWatchOnce(); // compact kind=watch NDJSON
const pack = locusVerifySession(); // full doctor + whoami + safe_next
// Soft annotation under LOCUS_ENFORCE=warn only (never hard-blocks alone)
const soft = locusSoftWatchHeartbeat(); // null when mode≠warn

// Or throw-style readiness (always probes)
ensureLocusReady();

// Merge locus into project MCP JSON
registerLocusInMcpConfig(".mcp.json", { client: "ashlr-hub" });

// Ephemeral CI pin — scrubbed env, no human active.json mutation
await withLocusSession("acme", async ({ env, sessionId }) => {
  const g = locusFleetGate(env);
  if (!g.allowDispatch) throw new Error(g.blockers.join("; "));
  // spawn children with env (includes LOCUS_SESSION_ID; no ambient tokens)
  return sessionId;
});

const { report, gateOk } = locusAgentReport();
const pin = parseStatusOneline(report?.status_oneline ?? "unpinned");
// pin.healthy && canMutate(report!.status, report!.status_oneline)

// ashlr doctor line
const line = locusDoctorLine(); // { id, ok, detail, fix? }
```

---

## Smoke

From the Locus repo:

```bash
export PATH="$HOME/.cargo/bin:$PATH"
./scripts/hub-smoke.sh
./scripts/hub-integration-test.sh
```

---

## Secrets policy (copy into hub CONTRIBUTING)

| Safe to read from locus JSON | Never from locus JSON |
|------------------------------|------------------------|
| Binding alias, tenant, session_id | Raw API keys, PATs |
| Credential presence/source metadata | Credential locator names and resolved values |
| `project_ref` / `team_id` scopes | Worker env secret maps |
| status / verdict / findings codes | Approval digests used as secrets |

If a secret lands in hub logs or agent context: **rotate it**; do not only redact later.
