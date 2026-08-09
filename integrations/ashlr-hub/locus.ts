/**
 * Drop-in Locus probe for ashlr-hub (or any orchestrator).
 *
 * Copy into hub as `src/core/integrations/locus.ts` (or import via path).
 *
 * SECURITY:
 *   - Never parse or persist secret VALUES from locus/phantom output.
 *   - credential_ref names (phm:NAME) are safe; resolved tokens are not.
 *   - Prefer REQUIRED_SERVERS = ["locus","phantom"] — never ambient supabase MCP.
 *
 * @see docs/hub-integration.md
 * @see schema/agent-report.schema.json
 */

import { execFileSync, spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const LOCUS_BIN = process.env.LOCUS_BIN ?? "locus";
const TIMEOUT_MS = 12_000;

/** Identity plane + secret plane — the Ashlr agent safety pair. */
export const REQUIRED_SERVERS = ["locus", "phantom"] as const;
export type RequiredServer = (typeof REQUIRED_SERVERS)[number];

export type LocusAgentExitCode = 0 | 1 | 2;
export type AgentStatus = "ready" | "protected" | "unsafe";
export type DoctorVerdict = "SAFE" | "WARN" | "UNSAFE";

// ---------------------------------------------------------------------------
// Types (keep aligned with schema/agent-report.schema.json)
// ---------------------------------------------------------------------------

export interface LocusAgentReport {
  version: string;
  ready: boolean;
  status: AgentStatus;
  status_oneline: string;
  home: string;
  pin: {
    pinned: boolean;
    alias?: string | null;
    tenant?: string | null;
    seal_ok?: boolean;
    expired?: boolean;
    frozen?: boolean;
  } | null;
  mcp_registered: {
    claude: boolean;
    cursor: boolean;
    codex: boolean;
  };
  doctor: unknown;
  commands: Record<string, string>;
  required_servers: string[];
  mcp_command: string;
  exit_code?: number;
  findings?: string[];
  next_steps?: string[];
  env_session_id?: string | null;
}

export interface LocusProbeResult {
  available: boolean;
  report: LocusAgentReport | null;
  exitCode: LocusAgentExitCode | number;
  error?: string;
  /** true when status is ready and oneline is not unpinned/frozen/invalid */
  gateOk: boolean;
}

// ---------------------------------------------------------------------------
// Probes
// ---------------------------------------------------------------------------

/** True when `locus` is on PATH. Never throws. */
export function locusAvailable(): boolean {
  try {
    execFileSync(
      process.platform === "win32" ? "where" : "which",
      [LOCUS_BIN],
      { stdio: "ignore", timeout: TIMEOUT_MS },
    );
    return true;
  } catch {
    return false;
  }
}

function locusEnv(extra?: NodeJS.ProcessEnv): NodeJS.ProcessEnv {
  return {
    ...process.env,
    LOCUS_HOME: process.env.LOCUS_HOME ?? join(homedir(), ".locus"),
    LOCUS_NOTIFY: "0",
    LOCUS_QUIET: "1",
    ...extra,
  };
}

/**
 * Run `locus agent report --json`.
 * Preferred hub readiness entrypoint.
 */
export function locusAgentReport(env?: NodeJS.ProcessEnv): LocusProbeResult {
  if (!locusAvailable()) {
    return {
      available: false,
      report: null,
      exitCode: 2,
      error: "locus CLI not found on PATH",
      gateOk: false,
    };
  }
  try {
    const r = spawnSync(LOCUS_BIN, ["agent", "report", "--json"], {
      encoding: "utf8",
      timeout: TIMEOUT_MS,
      env: locusEnv(env),
    });
    const exitCode = typeof r.status === "number" ? r.status : 2;
    const stdout = (r.stdout ?? "").trim();
    if (!stdout) {
      return {
        available: true,
        report: null,
        exitCode,
        error: r.stderr?.trim() || "empty agent report",
        gateOk: false,
      };
    }
    const report = JSON.parse(stdout) as LocusAgentReport;
    const oneline = report.status_oneline ?? "";
    const gateOk =
      report.ready === true &&
      report.status === "ready" &&
      !["unpinned", "frozen", "invalid"].includes(oneline);
    return { available: true, report, exitCode, gateOk };
  } catch (e) {
    return {
      available: true,
      report: null,
      exitCode: 2,
      error: e instanceof Error ? e.message : String(e),
      gateOk: false,
    };
  }
}

/** `locus status --oneline` — never throws. */
export function locusStatusOneline(env?: NodeJS.ProcessEnv): string {
  try {
    const r = spawnSync(LOCUS_BIN, ["status", "--oneline"], {
      encoding: "utf8",
      timeout: TIMEOUT_MS,
      env: locusEnv(env),
    });
    return (r.stdout ?? "").trim() || "unpinned";
  } catch {
    return "unpinned";
  }
}

/**
 * MCP server specs hub should register (names only — values never here).
 */
export function locusMcpServerSpecs(locusHome?: string): Record<
  string,
  { command: string; args: string[]; env: Record<string, string> }
> {
  const home = locusHome ?? process.env.LOCUS_HOME ?? join(homedir(), ".locus");
  return {
    locus: {
      command: "locus-mcp",
      args: [],
      env: {
        LOCUS_HOME: home,
        LOCUS_NOTIFY: "0",
        LOCUS_CLIENT: "ashlr-hub",
      },
    },
    // phantom registered by hub's ecosystem probe separately
  };
}

/**
 * Doctor line for `ashlr doctor` — names only, never secrets.
 */
export function locusDoctorLine(): {
  id: string;
  ok: boolean;
  detail: string;
  fix?: string;
} {
  const probe = locusAgentReport();
  if (!probe.available) {
    return {
      id: "locus",
      ok: false,
      detail: "locus CLI not installed",
      fix: "cargo install --git https://github.com/ashlrai/locus --package locus-cli --locked",
    };
  }
  if (!probe.report) {
    return {
      id: "locus",
      ok: false,
      detail: probe.error ?? "agent report failed",
      fix: "locus agent setup --apply --client all",
    };
  }
  const r = probe.report;
  const detail = `status=${r.status} pin=${r.status_oneline} ready=${r.ready}`;
  return {
    id: "locus",
    ok: probe.gateOk,
    detail,
    fix: probe.gateOk
      ? undefined
      : (r.next_steps?.[0] ?? "locus enter <alias> && locus agent setup --apply"),
  };
}

/** True when ~/.locus exists (initialized). */
export function locusHomeInitialized(): boolean {
  const home = process.env.LOCUS_HOME ?? join(homedir(), ".locus");
  return existsSync(home);
}
