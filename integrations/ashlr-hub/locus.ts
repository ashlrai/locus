/**
 * Drop-in Locus probe for ashlr-hub (or any orchestrator).
 *
 * Copy into hub as `src/core/integrations/locus.ts` (or import via path).
 *
 * SECURITY:
 *   - Never parse or persist secret VALUES from locus/phantom output.
 *   - credential_ref names (phm:NAME) are safe; resolved tokens are not.
 *   - Prefer REQUIRED_SERVERS = ["locus","phantom"] — never ambient supabase MCP.
 *   - withLocusSession uses `ci mint` (ephemeral); does not mutate active.json.
 *
 * @see docs/hub-integration.md
 * @see schema/agent-report.schema.json
 * @see integrations/ashlr-hub/mcp-gateway-snippet.md
 * @see integrations/ashlr-hub/doctor-check.md
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

/**
 * Tokens from `locus status --oneline`:
 *   unpinned | require_pin | frozen | invalid | alias:tenant
 */
export type StatusOnelineKind =
  | "unpinned"
  | "require_pin"
  | "frozen"
  | "invalid"
  | "pinned";

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
    pinned?: boolean;
    alias?: string | null;
    tenant?: string | null;
    binding_id?: string | null;
    seal_ok?: boolean;
    expired?: boolean;
    frozen?: boolean;
    expires_at?: string | null;
    principal?: string | null;
    client?: string | null;
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

/** Parsed form of `locus status --oneline`. */
export interface ParsedStatusOneline {
  kind: StatusOnelineKind;
  raw: string;
  alias?: string;
  tenant?: string;
  /** Healthy pin: `alias:tenant` (not frozen/invalid/unpinned/require_pin). */
  healthy: boolean;
}

/** Output of `locus ci mint -b <alias> --json` (secrets only if --resolve + env allow). */
export interface LocusCiMint {
  session_id: string;
  binding: string;
  binding_id: string;
  tenant: string;
  expires_at: string;
  seal: string;
  path: string;
  worker_home: string;
  secrets_resolved: boolean;
  env: Record<string, string>;
}

export interface WithLocusSessionOptions {
  /** Session TTL passed to `ci mint` (default 15m). */
  ttl?: string;
  /** Allow bindings outside workspace allowlist. */
  force?: boolean;
  /** Extra env merged into the child handle (mint LOCUS_* wins on conflict). */
  env?: NodeJS.ProcessEnv;
  /** Override LOCUS_HOME for mint + handle. */
  home?: string;
  /** Spawn timeout for mint (ms). */
  timeoutMs?: number;
}

export interface LocusSessionHandle {
  sessionId: string;
  binding: string;
  tenant: string;
  expiresAt: string;
  /** Env for children: LOCUS_HOME + LOCUS_SESSION_ID + mint env map. */
  env: NodeJS.ProcessEnv;
  mint: LocusCiMint;
}

export class LocusNotReadyError extends Error {
  readonly probe: LocusProbeResult;

  constructor(message: string, probe: LocusProbeResult) {
    super(message);
    this.name = "LocusNotReadyError";
    this.probe = probe;
  }
}

export class LocusMintError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "LocusMintError";
  }
}

// ---------------------------------------------------------------------------
// Env / process helpers
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

// ---------------------------------------------------------------------------
// Status oneline helpers
// ---------------------------------------------------------------------------

/**
 * Parse `locus status --oneline` tokens.
 *
 * | raw | kind | healthy |
 * |-----|------|---------|
 * | unpinned | unpinned | false |
 * | require_pin | require_pin | false |
 * | frozen | frozen | false |
 * | invalid | invalid | false |
 * | acme:acme-corp | pinned | true |
 */
export function parseStatusOneline(raw: string): ParsedStatusOneline {
  const s = (raw ?? "").trim() || "unpinned";
  if (s === "unpinned") {
    return { kind: "unpinned", raw: s, healthy: false };
  }
  if (s === "require_pin") {
    return { kind: "require_pin", raw: s, healthy: false };
  }
  if (s === "frozen") {
    return { kind: "frozen", raw: s, healthy: false };
  }
  if (s === "invalid") {
    return { kind: "invalid", raw: s, healthy: false };
  }
  const colon = s.indexOf(":");
  if (colon > 0 && colon < s.length - 1) {
    return {
      kind: "pinned",
      raw: s,
      alias: s.slice(0, colon),
      tenant: s.slice(colon + 1),
      healthy: true,
    };
  }
  // Unknown token → treat as invalid (fail closed)
  return { kind: "invalid", raw: s, healthy: false };
}

/** True when oneline is a healthy `alias:tenant` pin. */
export function isStatusOnelineHealthy(raw: string): boolean {
  return parseStatusOneline(raw).healthy;
}

/**
 * Hub mutate gate: ready status + healthy oneline.
 * Soft-blocks protected; hard-blocks unsafe / unpinned / frozen / invalid.
 */
export function canMutate(
  status: AgentStatus | string,
  statusOneline: string,
): boolean {
  if (status === "unsafe") return false;
  if (status !== "ready") return false;
  return isStatusOnelineHealthy(statusOneline);
}

// ---------------------------------------------------------------------------
// Probes
// ---------------------------------------------------------------------------

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
      maxBuffer: 4 * 1024 * 1024,
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
      isStatusOnelineHealthy(oneline);
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
 * Throw unless the identity plane is ready for mutating work.
 *
 * Fail closed: missing CLI, empty report, status≠ready, or unhealthy oneline.
 * Use before hub jobs that call tools / deploy / mutate infra.
 */
export function ensureLocusReady(env?: NodeJS.ProcessEnv): LocusAgentReport {
  const probe = locusAgentReport(env);
  if (!probe.available) {
    throw new LocusNotReadyError(
      probe.error ?? "locus CLI not found on PATH",
      probe,
    );
  }
  if (!probe.report) {
    throw new LocusNotReadyError(
      probe.error ?? "locus agent report failed",
      probe,
    );
  }
  const r = probe.report;
  const oneline = r.status_oneline ?? "unknown";
  if (r.status === "unsafe" || !probe.gateOk) {
    const hint =
      r.next_steps?.[0] ??
      "locus enter <alias> && locus agent setup --apply --client all";
    throw new LocusNotReadyError(
      `locus not ready: status=${r.status} pin=${oneline} — ${hint}`,
      probe,
    );
  }
  return r;
}

// ---------------------------------------------------------------------------
// CI mint / ephemeral session
// ---------------------------------------------------------------------------

/**
 * Mint a short-lived sealed CI session (`locus ci mint -b <binding>`).
 * Does not touch `active.json`. Never resolves secrets unless the CLI is
 * invoked with env that allows it (this helper does not pass `--resolve`).
 */
export function locusCiMint(
  binding: string,
  opts?: WithLocusSessionOptions,
): LocusCiMint {
  if (!binding?.trim()) {
    throw new LocusMintError("binding alias is required");
  }
  if (!locusAvailable()) {
    throw new LocusMintError("locus CLI not found on PATH");
  }
  const args = ["ci", "mint", "-b", binding.trim(), "--json"];
  if (opts?.ttl) {
    args.push("--ttl", opts.ttl);
  }
  if (opts?.force) {
    args.push("--force");
  }
  const env = locusEnv({
    ...opts?.env,
    ...(opts?.home ? { LOCUS_HOME: opts.home } : {}),
  });
  const r = spawnSync(LOCUS_BIN, args, {
    encoding: "utf8",
    timeout: opts?.timeoutMs ?? TIMEOUT_MS,
    env,
    maxBuffer: 4 * 1024 * 1024,
  });
  if (r.error) {
    throw new LocusMintError(`ci mint failed: ${r.error.message}`);
  }
  if (r.status !== 0) {
    throw new LocusMintError(
      `ci mint exit ${r.status}: ${(r.stderr ?? r.stdout ?? "").trim() || "unknown"}`,
    );
  }
  const stdout = (r.stdout ?? "").trim();
  if (!stdout) {
    throw new LocusMintError("ci mint returned empty stdout");
  }
  let mint: LocusCiMint;
  try {
    mint = JSON.parse(stdout) as LocusCiMint;
  } catch (e) {
    throw new LocusMintError(
      `ci mint JSON parse failed: ${e instanceof Error ? e.message : String(e)}`,
    );
  }
  if (!mint.session_id || !mint.env) {
    throw new LocusMintError("ci mint JSON missing session_id or env");
  }
  return mint;
}

/**
 * Run `fn` under an ephemeral Locus pin (`ci mint` → LOCUS_SESSION_ID).
 *
 * Use for hub job isolation so parallel agents do not share/mutate the human
 * shell pin (`active.json`). The mint env map is merged into `handle.env`
 * (scopes + LOCUS_* only — this helper never passes `--resolve`).
 *
 * @example
 * ```ts
 * await withLocusSession("acme", async ({ env, sessionId }) => {
 *   ensureLocusReady(env);
 *   // spawn workers / locus-mcp children with `env`
 *   return sessionId;
 * });
 * ```
 */
export async function withLocusSession<T>(
  binding: string,
  fn: (handle: LocusSessionHandle) => Promise<T> | T,
  opts?: WithLocusSessionOptions,
): Promise<T> {
  const mint = locusCiMint(binding, opts);
  const home =
    opts?.home ??
    process.env.LOCUS_HOME ??
    mint.env.LOCUS_HOME ??
    join(homedir(), ".locus");

  const env: NodeJS.ProcessEnv = {
    ...locusEnv(opts?.env),
    ...mint.env,
    LOCUS_HOME: home,
    LOCUS_SESSION_ID: mint.session_id,
    LOCUS_NOTIFY: "0",
    LOCUS_QUIET: "1",
  };

  const handle: LocusSessionHandle = {
    sessionId: mint.session_id,
    binding: mint.binding,
    tenant: mint.tenant,
    expiresAt: mint.expires_at,
    env,
    mint,
  };

  return await fn(handle);
}

// ---------------------------------------------------------------------------
// MCP / doctor integration helpers
// ---------------------------------------------------------------------------

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
 * @see integrations/ashlr-hub/doctor-check.md
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
