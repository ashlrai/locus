# ashlr-hub ↔ Locus drop-in

Notes and types for wiring **ashlr-hub** (or any agent orchestrator) to Locus without forking this repo.

Full contract: [`docs/hub-integration.md`](../../docs/hub-integration.md)  
Schemas: [`schema/agent-report.schema.json`](../../schema/agent-report.schema.json), [`schema/doctor.schema.json`](../../schema/doctor.schema.json)

---

## What hub should do

1. Shell out to Locus CLI (or spawn `locus-mcp` stdio) — do not reimplement pin/seal.
2. Prefer **`locus agent report --json`** as the single readiness probe.
3. Register MCP servers from **`required_servers`** (`locus` + `phantom` only).
4. **Never** parse or store secret values from locus/phantom output.

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

---

## CLI cheat sheet

```bash
export LOCUS_HOME="${LOCUS_HOME:-$HOME/.locus}"

locus agent report --json   # ★ hub entrypoint (exit 0|1|2)
locus doctor --json         # full mission-control pane (SAFE|WARN|UNSAFE)
locus whoami --json         # requires pin
locus status --oneline      # unpinned | alias:tenant | frozen | invalid
locus status --json
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
  /** Name only: phm:NAME | env:VAR — NEVER a resolved secret. */
  credential_ref: string;
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
  /** Phantom secret *names* only. */
  unresolved_phm: string[];
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

### Minimal hub probe helper

```ts
import { spawnSync } from "node:child_process";
import type { AgentReport, AgentStatus, LocusAgentExitCode } from "./types"; // your copy

export const REQUIRED_SERVERS = ["locus", "phantom"] as const;

export function locusAgentReport(env: NodeJS.ProcessEnv = process.env): {
  report: AgentReport;
  exitCode: LocusAgentExitCode;
} {
  const r = spawnSync("locus", ["agent", "report", "--json"], {
    encoding: "utf8",
    env,
    maxBuffer: 4 * 1024 * 1024,
  });
  const exitCode = (r.status ?? 1) as LocusAgentExitCode;
  if (!r.stdout?.trim()) {
    throw new Error(`locus agent report failed: ${r.stderr || r.error}`);
  }
  const report = JSON.parse(r.stdout) as AgentReport;
  // Hard rule: never treat credential_ref as a secret to inject into prompts.
  return { report, exitCode };
}

export function canMutate(status: AgentStatus, statusOneline: string): boolean {
  if (status === "unsafe") return false;
  if (["unpinned", "frozen", "invalid"].includes(statusOneline)) return false;
  return status === "ready";
}
```

---

## Smoke

From the Locus repo:

```bash
export PATH="$HOME/.cargo/bin:$PATH"
./scripts/hub-smoke.sh
```

---

## Secrets policy (copy into hub CONTRIBUTING)

| Safe to read from locus JSON | Never from locus JSON |
|------------------------------|------------------------|
| Binding alias, tenant, session_id | Raw API keys, PATs |
| `credential_ref` **names** (`phm:X`) | Resolved secret values |
| `project_ref` / `team_id` scopes | Worker env secret maps |
| status / verdict / findings codes | Approval digests used as secrets |

If a secret lands in hub logs or agent context: **rotate it**; do not only redact later.
