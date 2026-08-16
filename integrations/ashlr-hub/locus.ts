/**
 * Drop-in Locus probe for ashlr-hub (or any orchestrator).
 *
 * Copy into hub as `src/core/integrations/locus.ts` (or import via path).
 * Keep in sync with production hub features (scrubbed mint env, LOCUS_ENFORCE,
 * firm-mode config.locus.enforce / locus.firm, watch/verify session heartbeat —
 * hub #241 / #252 / #254 / #258 / #273).
 *
 * Hub-only (docs reference; do NOT port into this drop-in):
 *   - Firm onboard soft-offer (hub #274) — `ashlr onboard` / first enroll
 *   - Firm soft-warn checkLocusFirm (hub #277) — doctor/readiness only when
 *     enrolled>0 + locus available + locus.firm false; never hard-blocks mutate;
 *     monorepo firm default stays off. See docs/hub-integration.md +
 *     integrations/ashlr-hub/doctor-check.md; hub docs/LOCUS-FIRM-FLEET.md
 *
 * This file may be AHEAD of hub on capability/mint validators — do not regress
 * pure helpers when syncing. Hub doctor/readiness firm soft-warn is intentionally
 * out of scope here.
 *
 * SECURITY:
 *   - Never parse or persist secret VALUES from locus/phantom output.
 *   - Multi-tenant `lmt_` grant tokens are memory-only: they live inside a
 *     withLocusMcpTenant handle's headers for the callback's duration and are
 *     scrubbed afterwards — never logged, persisted, or placed in tool args.
 *   - Credential locators are private configuration; consume only presence/source metadata.
 *   - Prefer REQUIRED_SERVERS = ["locus","phantom"] — never ambient supabase MCP.
 *   - withLocusSession uses `ci mint` (ephemeral); does not mutate active.json.
 *   - Mint child env is scrubbed (no ambient credentials) + validateMintEnv allowlist.
 *
 * Pure (unit-testable) exports — no spawn / no FS:
 *   parseStatusOneline, isStatusOnelineHealthy, canMutate,
 *   parseRequiredServers, hasRequiredServers, parseAgentReportJson,
 *   parseWatchHeartbeat, parseSessionVerificationPack,
 *   blockersFromAgentReport, evaluateFleetGate, decidePreMutateGate,
 *   parseLocusEnforceToken, resolveLocusEnforceMode, extractLocusConfigEnforce,
 *   extractLocusConfigFirm, decideLocusSessionRun,
 *   scrubbedChildEnv, validateMintEnv, applyLocusSessionEnv, parseMcpConfigJson,
 *   mergeLocusIntoMcpConfig, locusServerSpec,
 *   parseMcpMintOutput, parseMcpListOutput, classifyTenantAuthError
 *
 * Shell-out / FS: locusAvailable, locusAgentReport, locusVerifySession,
 *   locusWatchOnce, locusSoftWatchHeartbeat, ensureLocusReady, locusFleetGate,
 *   assertLocusPreMutate, applyLocusPreMutateGate, withLocusSession,
 *   runWithLocusSessionIfConfigured, locusDoctorLine, registerLocusInMcpConfig,
 *   readLocusConfigFromAshlr,
 *   locusMcpMint, locusMcpList, locusMcpRevoke, withLocusMcpTenant
 *
 * HTTP (global fetch — no deps): locusMtPreflight
 *
 * Fleet heartbeat (session pack / watch tick — never secrets):
 *   locus verify session --json  → full doctor + whoami + safe_next pack
 *   locus watch --once --json    → compact NDJSON tick (kind=watch)
 *   Soft only under LOCUS_ENFORCE=warn (readiness/doctor) — never hard-blocks alone.
 *
 * Pre-mutate enforcement (opt-in — never always-on):
 *   Resolution order for mode (see resolveLocusEnforceMode):
 *     1. env LOCUS_ENFORCE when set (wins over config, including LOCUS_ENFORCE=off)
 *     2. ~/.ashlr/config.json → locus.enforce ("off"|"warn"|"enforce") when set
 *     3. locus.firm === true → enforce (production fleet profile)
 *     4. off (monorepo-safe default when config/env unset)
 *
 *   Tokens (env or config.enforce):
 *     1|true|yes|enforce|block → enforce (fail closed when fleet gate blocks)
 *     warn|log                 → warn (log blockers, allow dispatch)
 *     unset / 0 / false / no / off / "" → off (no CLI probe)
 *     unknown non-empty env token → enforce (fail closed; do not soft-allow typos)
 *
 *   Firm profile (production fleets — opt-in, never monorepo default):
 *     { "locus": { "firm": true } }
 *     # ashlr config set locus.firm true
 *   Explicit enforce mode (overrides firm when set):
 *     { "locus": { "enforce": "enforce" } }
 *   Soft roll-out:
 *     { "locus": { "enforce": "warn" } }
 *   Local override without editing config:
 *     LOCUS_ENFORCE=off
 *
 * CI session isolation (runWithLocusSessionIfConfigured):
 *   LOCUS_CI_BINDING / LOCUS_BINDING → ephemeral `ci mint` (no ambient active.json)
 *   enforce mode without binding → refuse (LocusSessionConfigError)
 *   warn mode without binding → warn + pass-through
 *   LOCUS_SESSION_ID already set → already-session (skip re-mint)
 *   otherwise → pass-through (monorepo-safe default)
 *
 * Call sites (opt-in only — never always-on; hub production wire-in):
 *   - spawnEngine / runSwarmInternal / runApiModelSandboxed — pre-mutate gate
 *   - runSwarm / runTask — CI session mint overlay (fleet + single-task paths; hub #252)
 *   - simple-conductor / runBestOfN — same CI session mint overlay (hub #256 / #257)
 *
 * @see docs/hub-integration.md
 * @see schema/agent-report.schema.json
 * @see schema/hub-gate.schema.json
 * @see integrations/ashlr-hub/mcp-gateway-snippet.md
 * @see integrations/ashlr-hub/doctor-check.md
 * @see integrations/ashlr-hub/fleet-preflight.md
 */

import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, realpathSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, isAbsolute, join, relative, resolve } from "node:path";

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const LOCUS_BIN = process.env.LOCUS_BIN ?? "locus";
const TIMEOUT_MS = 12_000;
const CONTROL_CAPABILITY_ENV = "LOCUS_CONTROL_CAPABILITY";
const EXECUTOR_CAPABILITY_ENV = "LOCUS_EXECUTOR_CAPABILITY";

/** Identity plane + secret plane — the Ashlr agent safety pair. */
export const REQUIRED_SERVERS = ["locus", "phantom"] as const;
export type RequiredServer = (typeof REQUIRED_SERVERS)[number];

/** MCP multiplexor binary — never raw provider MCPs. */
export const MCP_COMMAND = "locus-mcp" as const;

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
// Types (keep aligned with schema/agent-report.schema.json + hub-gate.schema.json)
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

/**
 * Compact NDJSON tick from `locus watch --once --json` (hub fleet heartbeat).
 * Aliases / verdicts only — never secrets.
 * @see docs/verification-plane.md
 */
export interface LocusWatchHeartbeat {
  /** Stable stream tag (`watch`). Legacy runtime ticks may omit this. */
  kind: "watch";
  /** True when doctor ok and safe_next.ready (same gate as session pack). */
  session_ok: boolean;
  /** Active binding alias when whoami is available. */
  whoami?: string | null;
  /** Doctor verdict: SAFE | WARN | UNSAFE (or unknown for legacy). */
  doctor_verdict: string;
  /** Machine-readable safe_next action (ready | enter | re_pin | …). */
  safe_next: string;
  /** Whether a pin is currently present. */
  pinned: boolean;
  /** Runtime frozen (binding drift under live session). */
  frozen: boolean;
  /**
   * Source shape that was parsed.
   * - `watch`: modern `kind=watch` tick
   * - `legacy-runtime`: older drift object (`ok` / `binding_alias` / …)
   */
  source?: "watch" | "legacy-runtime";
}

/** Result of `locusWatchOnce` (never throws). */
export interface LocusWatchProbeResult {
  available: boolean;
  heartbeat: LocusWatchHeartbeat | null;
  exitCode: number;
  error?: string;
  /** True when heartbeat.session_ok. */
  ok: boolean;
}

/**
 * Values-free subset of `locus verify session --json` for hub heartbeats.
 * Nested doctor/whoami objects are kept as opaque records (names/verdicts only
 * when consumed) — never secret values.
 */
export interface LocusSessionVerificationPack {
  kind: "session";
  version: string;
  whoami?: Record<string, unknown> | null;
  doctor?: Record<string, unknown> | null;
  safe_next?: {
    action?: string;
    ready?: boolean;
    message?: string;
    command?: string;
    [key: string]: unknown;
  } | null;
  session_ok: boolean;
  [key: string]: unknown;
}

/** Result of `locusVerifySession` (never throws). */
export interface LocusSessionVerifyResult {
  available: boolean;
  pack: LocusSessionVerificationPack | null;
  exitCode: number;
  error?: string;
  /** True when pack.session_ok. */
  sessionOk: boolean;
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

/**
 * Fleet pre-dispatch gate result.
 * @see schema/hub-gate.schema.json
 * @see integrations/ashlr-hub/fleet-preflight.md
 */
export interface LocusFleetGateResult {
  /** True only when every preflight check passes (fail closed). */
  allowDispatch: boolean;
  /** Human-readable blockers; empty iff allowDispatch. */
  blockers: string[];
  /** Agent report when available; null if CLI missing / parse failure. */
  report: LocusAgentReport | null;
  /** Echo of probe availability (CLI on PATH). */
  available?: boolean;
  /** Echo of canMutate/gateOk when report present. */
  gateOk?: boolean;
  status?: AgentStatus | string;
  status_oneline?: string;
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
  /** Explicitly authorized env merged into the scrubbed child handle. */
  env?: NodeJS.ProcessEnv;
  /** Override LOCUS_HOME for mint + handle. */
  home?: string;
  /** Spawn timeout for mint (ms). */
  timeoutMs?: number;
}

/** How the hub should treat locus fleet-gate blockers before mutate/dispatch. */
export type LocusEnforceMode = "off" | "warn" | "enforce";

/**
 * Firm-mode locus slice from `~/.ashlr/config.json` (or hub `AshlrConfig.locus`).
 * After hub #254/#258: `enforce` and/or `firm`; extra keys are ignored.
 */
export interface LocusFirmConfig {
  /**
   * Pre-mutate / CI session isolation mode.
   * - off: no CLI probe (default when unset)
   * - warn: log blockers, allow dispatch
   * - enforce: fail closed when fleet gate blocks
   * Explicit value beats `firm` profile.
   */
  enforce?: LocusEnforceMode | string;
  /**
   * Firm profile for production fleets. When true and `enforce` is unset
   * (and env LOCUS_ENFORCE is unset), mode resolves to enforce.
   * Default false/absent — monorepo stays off. Hub #258.
   */
  firm?: boolean;
}

/**
 * Optional config slice consulted by {@link resolveLocusEnforceMode} after env.
 * Accepts either `cfg.locus`, a bare `{ enforce?, firm? }`, full hub
 * AshlrConfig-shaped objects, or null/undefined.
 */
export type LocusEnforceConfigInput =
  | { locus?: LocusFirmConfig | null; [key: string]: unknown }
  | LocusFirmConfig
  | null
  | undefined;

/**
 * Decision from the pre-mutate gate wrapper.
 * Pure when built via `decidePreMutateGate`; shell-out when via `assertLocusPreMutate`.
 */
export interface LocusPreMutateDecision {
  /** False only when mode=enforce and fleet gate would block. */
  allow: boolean;
  mode: LocusEnforceMode;
  /** Human-readable blockers (never secrets). Empty when healthy or mode=off. */
  blockers: string[];
  gateOk?: boolean;
  status_oneline?: string;
  status?: AgentStatus | string;
  available?: boolean;
  /** True when mode is warn and blockers present (caller may log). */
  shouldWarn: boolean;
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

interface LocusWhoami {
  session_id: string;
  binding_alias: string;
  binding_id: string;
  tenant: string;
  expires_at: string;
  worker_home: string;
  seal_ok: boolean;
  seal: string;
  authority: string;
  authority_anchor_ok: boolean;
  backing_type: string;
  backing_path: string;
  frozen?: boolean;
  providers?: Array<{
    provider: string;
    account: string;
    project_ref?: string | null;
    team_id?: string | null;
    account_id?: string | null;
    read_only?: boolean | null;
    orgs?: string[];
    repos?: string[];
  }>;
}

/** Claude / Cursor style MCP config root. */
export interface McpConfigJson {
  mcpServers?: Record<string, McpServerEntry>;
  [key: string]: unknown;
}

export interface McpServerEntry {
  command: string;
  args?: string[];
  env?: Record<string, string>;
  [key: string]: unknown;
}

export interface MergeLocusMcpOptions {
  /** Effective LOCUS_HOME injected into server env. */
  locusHome?: string;
  /** LOCUS_CLIENT value (default ashlr-hub). */
  client?: string;
  /** Optional LOCUS_SESSION_ID for ephemeral pin. */
  sessionId?: string;
  /** Server name key (default "locus"). */
  name?: string;
  /** Binary (default locus-mcp). */
  command?: string;
}

export interface MergeLocusMcpResult {
  config: McpConfigJson;
  /** True when mcpServers.locus (or name) was inserted or materially changed. */
  changed: boolean;
  serverName: string;
}

export interface RegisterLocusMcpResult extends MergeLocusMcpResult {
  path: string;
  /** True when the file was written. */
  written: boolean;
  created: boolean;
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

export class LocusMcpConfigError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "LocusMcpConfigError";
  }
}

/** Thrown when LOCUS_ENFORCE requires a CI binding but none is configured. */
export class LocusSessionConfigError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "LocusSessionConfigError";
  }
}

/**
 * Pure decision for {@link runWithLocusSessionIfConfigured}.
 * Never shells out; never throws.
 */
export type LocusSessionRunDecision =
  | {
      /** Mint ephemeral session via `locus ci mint -b <binding>`. */
      kind: "mint";
      binding: string;
      /** Env key that supplied the binding alias. */
      source: "LOCUS_CI_BINDING" | "LOCUS_BINDING";
    }
  | {
      /** Already under a sealed session id — skip re-mint. */
      kind: "already-session";
      sessionId: string;
    }
  | {
      /** No binding and enforce mode — fail closed. */
      kind: "refuse";
      reason: string;
      mode: LocusEnforceMode;
    }
  | {
      /** No binding and warn mode — allow ambient env with a warning. */
      kind: "warn";
      reason: string;
      mode: LocusEnforceMode;
    }
  | {
      /** No binding and enforce off — monorepo-safe pass-through. */
      kind: "pass-through";
      mode: LocusEnforceMode;
    };

export interface RunWithLocusSessionOptions extends WithLocusSessionOptions {
  /** Env consulted for LOCUS_CI_BINDING / LOCUS_BINDING / LOCUS_ENFORCE. */
  env?: NodeJS.ProcessEnv;
  /** Called once when decision is warn (default: stderr). */
  onWarn?: (message: string) => void;
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
  const source = { ...process.env, ...extra };
  const clean = scrubbedChildEnv(source);
  for (const key of [
    "LOCUS_HOME",
    "LOCUS_SESSION_ID",
    CONTROL_CAPABILITY_ENV,
    EXECUTOR_CAPABILITY_ENV,
  ]) {
    const value = source[key];
    if (typeof value === "string" && value) clean[key] = value;
  }
  return {
    ...clean,
    LOCUS_HOME: source.LOCUS_HOME ?? join(homedir(), ".locus"),
    LOCUS_NOTIFY: "0",
    LOCUS_QUIET: "1",
  };
}

const CHILD_RUNTIME_ENV = new Set([
  "PATH",
  "HOME",
  "USER",
  "LOGNAME",
  "SHELL",
  "TMPDIR",
  "TMP",
  "TEMP",
  "LANG",
  "TERM",
  "SystemRoot",
  "WINDIR",
  "PATHEXT",
  "ComSpec",
]);

const MINT_SCOPE_ENV = new Set([
  "GH_CONFIG_DIR",
  "AWS_CONFIG_FILE",
  "AWS_SHARED_CREDENTIALS_FILE",
  "AWS_ACCOUNT_ID",
  "CLOUDFLARE_ACCOUNT_ID",
  "SUPABASE_PROJECT_ID",
  "SUPABASE_PROJECT_REF",
  "VERCEL_ORG_ID",
  "VERCEL_PROJECT_ID",
  "VERCEL_TEAM_ID",
]);

const MINT_IDENTITY_ENV = new Set([
  "LOCUS_SESSION_ID",
  "LOCUS_BINDING",
  "LOCUS_BINDING_ID",
  "LOCUS_TENANT",
  "LOCUS_PRINCIPAL",
  "LOCUS_SEAL",
  "LOCUS_WORKER_HOME",
  "LOCUS_EXPIRES_AT",
  "LOCUS_PROVIDERS",
  EXECUTOR_CAPABILITY_ENV,
]);

function isAllowedMintEnvKey(key: string): boolean {
  return (
    MINT_IDENTITY_ENV.has(key) ||
    MINT_SCOPE_ENV.has(key) ||
    /^LOCUS_[A-Z0-9_]+_(?:ACCOUNT|CREDENTIAL_RESOLVED|PROJECT_REF|TEAM_ID|ACCOUNT_ID|READ_ONLY|ORGS|REPOS|PROJECTS)$/.test(
      key,
    )
  );
}

/** Build a child baseline without inheriting ambient credentials. */
export function scrubbedChildEnv(
  parent: NodeJS.ProcessEnv = process.env,
  explicit?: NodeJS.ProcessEnv,
): NodeJS.ProcessEnv {
  const clean: NodeJS.ProcessEnv = {};
  for (const [key, value] of Object.entries(parent)) {
    if (CHILD_RUNTIME_ENV.has(key) || key.startsWith("LC_")) {
      clean[key] = value;
    }
  }
  const merged = { ...clean, ...explicit };
  // Operator control is never delegated into Hub callbacks or their children.
  delete merged[CONTROL_CAPABILITY_ENV];
  return merged;
}

/** Validate the non-secret identity/scope environment emitted by `ci mint`. */
export function validateMintEnv(raw: unknown): Record<string, string> {
  if (raw === null || typeof raw !== "object" || Array.isArray(raw)) {
    throw new LocusMintError("ci mint JSON has invalid env map");
  }
  const clean: Record<string, string> = {};
  for (const [key, value] of Object.entries(raw)) {
    if (
      !isAllowedMintEnvKey(key) ||
      typeof value !== "string" ||
      /(?:phm|env|test):/i.test(value)
    ) {
      throw new LocusMintError("ci mint JSON contains disallowed env metadata");
    }
    clean[key] = value;
  }
  const executor = clean[EXECUTOR_CAPABILITY_ENV];
  if (!executor || !/^[a-f0-9]{64}$/i.test(executor)) {
    throw new LocusMintError("ci mint JSON has invalid executor authority");
  }
  return clean;
}

/**
 * Bind one Hub mint request to exactly one sealed session identity.
 * Descriptive env labels are accepted only when they exactly mirror the
 * top-level, CLI-sealed mint response; they never select a different binding.
 */
export function validateMintBinding(
  requestedBinding: string,
  mint: LocusCiMint,
): LocusCiMint {
  const requested = requestedBinding.trim();
  if (!requested || (mint.binding !== requested && mint.binding_id !== requested)) {
    throw new LocusMintError("ci mint returned a different binding than requested");
  }
  const expected: Record<string, string> = {
    LOCUS_SESSION_ID: mint.session_id,
    LOCUS_BINDING: mint.binding,
    LOCUS_BINDING_ID: mint.binding_id,
    LOCUS_TENANT: mint.tenant,
    LOCUS_SEAL: mint.seal,
    LOCUS_WORKER_HOME: mint.worker_home,
    LOCUS_EXPIRES_AT: mint.expires_at,
  };
  for (const [key, value] of Object.entries(expected)) {
    if (!value || mint.env[key] !== value) {
      throw new LocusMintError("ci mint identity environment does not match sealed response");
    }
  }
  return mint;
}

function verifiedSessionEnv(
  source: NodeJS.ProcessEnv,
  home: string,
  whoami: LocusWhoami,
): NodeJS.ProcessEnv {
  const env = scrubbedChildEnv(source);
  const executor = source[EXECUTOR_CAPABILITY_ENV];
  if (!executor || !/^[a-f0-9]{64}$/i.test(executor)) {
    throw new LocusMintError("existing session lacks live executor authority");
  }
  Object.assign(env, {
    LOCUS_HOME: home,
    LOCUS_SESSION_ID: whoami.session_id,
    LOCUS_BINDING: whoami.binding_alias,
    LOCUS_BINDING_ID: whoami.binding_id,
    LOCUS_TENANT: whoami.tenant,
    LOCUS_SEAL: whoami.seal,
    LOCUS_WORKER_HOME: whoami.worker_home,
    LOCUS_EXPIRES_AT: whoami.expires_at,
    LOCUS_PROVIDERS: (whoami.providers ?? []).map((p) => p.provider).join(","),
    LOCUS_EXECUTOR_CAPABILITY: executor,
    LOCUS_NOTIFY: "0",
    LOCUS_QUIET: "1",
    HOME: whoami.worker_home,
    USERPROFILE: whoami.worker_home,
    GH_CONFIG_DIR: join(whoami.worker_home, "gh"),
    AWS_CONFIG_FILE: join(whoami.worker_home, "aws", "config"),
    AWS_SHARED_CREDENTIALS_FILE: join(whoami.worker_home, "aws", "credentials"),
  });

  for (const provider of whoami.providers ?? []) {
    const prefix = `LOCUS_${provider.provider.toUpperCase().replace(/[^A-Z0-9]/g, "_")}`;
    env[`${prefix}_ACCOUNT`] = provider.account;
    env[`${prefix}_CREDENTIAL_RESOLVED`] = "0";
    if (provider.project_ref) env[`${prefix}_PROJECT_REF`] = provider.project_ref;
    if (provider.team_id) env[`${prefix}_TEAM_ID`] = provider.team_id;
    if (provider.account_id) env[`${prefix}_ACCOUNT_ID`] = provider.account_id;
    if (typeof provider.read_only === "boolean") {
      env[`${prefix}_READ_ONLY`] = String(provider.read_only);
    }
    if (provider.orgs?.length) env[`${prefix}_ORGS`] = provider.orgs.join(",");
    if (provider.repos?.length) env[`${prefix}_REPOS`] = provider.repos.join(",");
  }
  delete env[CONTROL_CAPABILITY_ENV];
  return env;
}

/**
 * Verify an inherited Hub session against its exact sealed backing and live
 * broker lease. Descriptive LOCUS_* labels must exactly match the authenticated
 * record; they never confer authority by themselves.
 */
export function validateExistingLocusSession(
  source: NodeJS.ProcessEnv = process.env,
  mint?: LocusCiMint,
): LocusSessionHandle {
  const sessionId = (source.LOCUS_SESSION_ID ?? "").trim();
  const executor = (source[EXECUTOR_CAPABILITY_ENV] ?? "").trim();
  if (!/^ses_[a-f0-9]+$/i.test(sessionId) || !/^[a-f0-9]{64}$/i.test(executor)) {
    throw new LocusMintError("existing session identity or executor authority is invalid");
  }
  const home = source.LOCUS_HOME ?? join(homedir(), ".locus");
  let canonicalHome: string;
  try {
    canonicalHome = realpathSync(home);
  } catch {
    throw new LocusMintError("existing session LOCUS_HOME is unavailable");
  }
  const commandEnv: NodeJS.ProcessEnv = {
    ...scrubbedChildEnv(source),
    LOCUS_HOME: home,
    LOCUS_SESSION_ID: sessionId,
    LOCUS_EXECUTOR_CAPABILITY: executor,
    LOCUS_NOTIFY: "0",
    LOCUS_QUIET: "1",
  };
  const result = spawnSync(LOCUS_BIN, ["whoami", "--json"], {
    encoding: "utf8",
    timeout: TIMEOUT_MS,
    env: commandEnv,
    maxBuffer: 1024 * 1024,
  });
  if (result.error || result.status !== 0) {
    throw new LocusMintError("existing session failed live authority verification");
  }

  let whoami: LocusWhoami;
  try {
    whoami = JSON.parse((result.stdout ?? "").trim()) as LocusWhoami;
  } catch {
    throw new LocusMintError("existing session verification returned invalid JSON");
  }
  const expiry = Date.parse(whoami.expires_at);
  const expectedBacking = join(canonicalHome, "sessions", `ci-${sessionId.slice(4)}.json`);
  const expectedWorker = join(canonicalHome, "workers", sessionId);
  const backingRelative = relative(
    resolve(canonicalHome, "sessions"),
    resolve(whoami.backing_path),
  );
  if (
    whoami.session_id !== sessionId ||
    !whoami.seal_ok ||
    !whoami.authority_anchor_ok ||
    whoami.authority !== "delegated" ||
    whoami.backing_type !== "ci" ||
    whoami.frozen === true ||
    !Number.isFinite(expiry) ||
    expiry <= Date.now() ||
    !isAbsolute(whoami.backing_path) ||
    backingRelative.startsWith("..") ||
    isAbsolute(backingRelative) ||
    resolve(whoami.backing_path) !== resolve(expectedBacking) ||
    resolve(whoami.worker_home) !== resolve(expectedWorker)
  ) {
    throw new LocusMintError("existing session authority, backing, or expiry is invalid");
  }

  const expectedLabels: Record<string, string> = {
    LOCUS_BINDING: whoami.binding_alias,
    LOCUS_BINDING_ID: whoami.binding_id,
    LOCUS_TENANT: whoami.tenant,
    LOCUS_SEAL: whoami.seal,
    LOCUS_WORKER_HOME: whoami.worker_home,
    LOCUS_EXPIRES_AT: whoami.expires_at,
    LOCUS_PROVIDERS: (whoami.providers ?? []).map((p) => p.provider).join(","),
  };
  for (const [key, expected] of Object.entries(expectedLabels)) {
    const supplied = source[key];
    if (typeof supplied === "string" && supplied !== expected) {
      throw new LocusMintError(`existing session ${key} label does not match authority`);
    }
  }

  const env = verifiedSessionEnv(source, canonicalHome, whoami);
  const verifiedMint: LocusCiMint = mint ?? {
    session_id: whoami.session_id,
    binding: whoami.binding_alias,
    binding_id: whoami.binding_id,
    tenant: whoami.tenant,
    expires_at: whoami.expires_at,
    seal: whoami.seal,
    path: whoami.backing_path,
    worker_home: whoami.worker_home,
    secrets_resolved: false,
    env: Object.fromEntries(
      Object.entries(env).filter(
        (entry): entry is [string, string] => typeof entry[1] === "string",
      ),
    ),
  };
  return {
    sessionId: whoami.session_id,
    binding: whoami.binding_alias,
    tenant: whoami.tenant,
    expiresAt: whoami.expires_at,
    env,
    mint: verifiedMint,
  };
}

/**
 * Merge sealed-session identity/scope keys from a mint handle into `target`.
 * Pure: mutates and returns `target`. Never copies secrets (allowlist only).
 *
 * Also accepts LOCUS_HOME / LOCUS_NOTIFY / LOCUS_QUIET from withLocusSession.
 */
export function applyLocusSessionEnv(
  target: NodeJS.ProcessEnv,
  sessionEnv: NodeJS.ProcessEnv | Record<string, string>,
): NodeJS.ProcessEnv {
  for (const [key, value] of Object.entries(sessionEnv)) {
    if (typeof value !== "string") continue;
    if (
      key === "LOCUS_HOME" ||
      key === "LOCUS_NOTIFY" ||
      key === "LOCUS_QUIET" ||
      isAllowedMintEnvKey(key)
    ) {
      target[key] = value;
    }
  }
  return target;
}

// ---------------------------------------------------------------------------
// Status oneline helpers (pure)
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
// Required servers + report parse helpers (pure)
// ---------------------------------------------------------------------------

/**
 * Normalize unknown JSON into a string[] of server names.
 * Non-arrays / non-strings are dropped (fail closed → empty).
 */
export function parseRequiredServers(servers: unknown): string[] {
  if (!Array.isArray(servers)) return [];
  const out: string[] = [];
  for (const s of servers) {
    if (typeof s === "string" && s.trim()) {
      out.push(s.trim());
    }
  }
  return out;
}

/**
 * True when the list includes both REQUIRED_SERVERS entries (`locus` + `phantom`).
 */
export function hasRequiredServers(
  servers: string[] | readonly string[] | null | undefined,
): boolean {
  if (!servers || servers.length === 0) return false;
  const set = new Set(
    [...servers].map((s) => String(s).trim().toLowerCase()).filter(Boolean),
  );
  return REQUIRED_SERVERS.every((r) => set.has(r));
}

/**
 * Parse agent-report JSON text. Throws on invalid JSON / non-object.
 * Does not validate every schema field — hub may tolerate extra keys.
 */
export function parseAgentReportJson(raw: string): LocusAgentReport {
  const text = (raw ?? "").trim();
  if (!text) {
    throw new Error("empty agent report JSON");
  }
  const v = JSON.parse(text) as unknown;
  if (v === null || typeof v !== "object" || Array.isArray(v)) {
    throw new Error("agent report root is not an object");
  }
  return v as LocusAgentReport;
}

/**
 * Parse `locus verify session --json` pack. Throws on invalid JSON / non-object.
 * Does not deep-validate doctor/whoami — hub only needs session_ok + shape.
 */
export function parseSessionVerificationPack(
  raw: string,
): LocusSessionVerificationPack {
  const text = (raw ?? "").trim();
  if (!text) {
    throw new Error("empty session verification JSON");
  }
  const v = JSON.parse(text) as unknown;
  if (v === null || typeof v !== "object" || Array.isArray(v)) {
    throw new Error("session verification root is not an object");
  }
  const obj = v as Record<string, unknown>;
  const sessionOk =
    obj.session_ok === true ||
    obj.session_ok === "true" ||
    obj.session_ok === 1;
  return {
    ...(obj as LocusSessionVerificationPack),
    kind: "session",
    version: typeof obj.version === "string" ? obj.version : "",
    session_ok: sessionOk,
  };
}

/**
 * Pure: parse a `locus watch --once --json` NDJSON tick into a heartbeat.
 *
 * Accepts:
 *   1. Modern pack: `{ kind:"watch", session_ok, whoami?, doctor_verdict, safe_next, pinned, frozen }`
 *   2. Legacy runtime drift: `{ ok, pinned, frozen, binding_alias?, seal_ok?, issues? }`
 *      (installed locus 0.2.0 and earlier) — mapped to the same hub shape.
 *
 * Never throws for unknown fields. Throws only on empty / invalid JSON /
 * non-object root (fail closed — caller treats as probe error).
 *
 * Secrets: never expected; whoami is alias-only.
 */
export function parseWatchHeartbeat(
  raw: string | Record<string, unknown>,
): LocusWatchHeartbeat {
  let obj: Record<string, unknown>;
  if (typeof raw === "string") {
    const text = raw.trim();
    if (!text) {
      throw new Error("empty watch heartbeat JSON");
    }
    // NDJSON may be multi-line continuous stream; take the last non-empty line.
    const line =
      text
        .split(/\r?\n/)
        .map((l) => l.trim())
        .filter(Boolean)
        .pop() ?? text;
    const v = JSON.parse(line) as unknown;
    if (v === null || typeof v !== "object" || Array.isArray(v)) {
      throw new Error("watch heartbeat root is not an object");
    }
    obj = v as Record<string, unknown>;
  } else if (raw !== null && typeof raw === "object" && !Array.isArray(raw)) {
    obj = raw;
  } else {
    throw new Error("watch heartbeat root is not an object");
  }

  const kindRaw =
    typeof obj.kind === "string" ? obj.kind.trim().toLowerCase() : "";

  // Modern kind=watch (or session pack re-used as tick via session_ok fields)
  if (
    kindRaw === "watch" ||
    ("session_ok" in obj &&
      ("doctor_verdict" in obj || "safe_next" in obj || kindRaw === "watch"))
  ) {
    const sessionOk = coerceBool(obj.session_ok);
    const whoami = coerceOptionalString(obj.whoami);
    const doctorVerdict =
      coerceOptionalString(obj.doctor_verdict) ??
      coerceOptionalString(obj.doctorVerdict) ??
      "unknown";
    const safeNext =
      coerceSafeNextAction(obj.safe_next) ??
      coerceSafeNextAction(obj.safeNext) ??
      "unknown";
    const pinned =
      coerceBool(obj.pinned) || (whoami != null && whoami.length > 0);
    const frozen = coerceBool(obj.frozen);
    return {
      kind: "watch",
      session_ok: sessionOk,
      whoami: whoami ?? null,
      doctor_verdict: doctorVerdict,
      safe_next: safeNext,
      pinned,
      frozen,
      source: "watch",
    };
  }

  // Legacy runtime drift object (pre-session-pack watch)
  if (
    "ok" in obj ||
    "binding_alias" in obj ||
    "binding_present" in obj ||
    "seal_ok" in obj
  ) {
    const ok = coerceBool(obj.ok);
    const pinned = coerceBool(obj.pinned) || coerceBool(obj.binding_present);
    const frozen = coerceBool(obj.frozen);
    const whoami =
      coerceOptionalString(obj.binding_alias) ??
      coerceOptionalString(obj.whoami) ??
      null;
    const issues = Array.isArray(obj.issues)
      ? obj.issues.filter((i): i is string => typeof i === "string")
      : [];
    const safeNext = pinned
      ? ok
        ? "ready"
        : (issues[0] ?? "re_pin")
      : "enter";
    return {
      kind: "watch",
      session_ok: ok && pinned && !frozen,
      whoami,
      doctor_verdict:
        ok && pinned && !frozen ? "SAFE" : frozen ? "UNSAFE" : "WARN",
      safe_next: safeNext,
      pinned,
      frozen,
      source: "legacy-runtime",
    };
  }

  throw new Error("unrecognized watch heartbeat shape");
}

function coerceBool(v: unknown): boolean {
  if (v === true || v === 1 || v === "1" || v === "true") return true;
  return false;
}

function coerceOptionalString(v: unknown): string | undefined {
  if (typeof v === "string" && v.trim()) return v.trim();
  return undefined;
}

/** Accept string action or nested `{ action }` safe_next object. */
function coerceSafeNextAction(v: unknown): string | undefined {
  if (typeof v === "string" && v.trim()) return v.trim();
  if (v !== null && typeof v === "object" && !Array.isArray(v)) {
    const action = (v as { action?: unknown }).action;
    if (typeof action === "string" && action.trim()) return action.trim();
  }
  return undefined;
}

/**
 * Stable keys every hub consumer should see on agent report JSON.
 * Keep aligned with AGENT_REPORT_JSON_KEYS in locus-core.
 */
export const AGENT_REPORT_STABLE_KEYS = [
  "version",
  "ready",
  "status",
  "mcp_registered",
  "doctor",
  "commands",
  "exit_code",
  "status_oneline",
  "home",
  "required_servers",
  "mcp_command",
] as const;

/** Return missing stable keys (empty when complete). */
export function missingAgentReportKeys(report: unknown): string[] {
  if (report === null || typeof report !== "object" || Array.isArray(report)) {
    return ["root is not an object"];
  }
  const obj = report as Record<string, unknown>;
  return AGENT_REPORT_STABLE_KEYS.filter((k) => !(k in obj));
}

// ---------------------------------------------------------------------------
// Fleet gate (pure evaluation + shell-out wrapper)
// ---------------------------------------------------------------------------

export interface EvaluateFleetGateOptions {
  /**
   * When true (default), also require required_servers ⊇ {locus, phantom}
   * and mcp_command === locus-mcp.
   */
  requireHubContract?: boolean;
}

/**
 * Pure: compute human-readable blockers from an agent report (or null).
 * Never throws. Fail closed when report is missing.
 */
export function blockersFromAgentReport(
  report: LocusAgentReport | null | undefined,
  opts?: EvaluateFleetGateOptions,
): string[] {
  const requireContract = opts?.requireHubContract !== false;
  const blockers: string[] = [];

  if (!report) {
    blockers.push("no agent report");
    return blockers;
  }

  const status = report.status ?? "unknown";
  const oneline = report.status_oneline ?? "unpinned";
  const parsed = parseStatusOneline(oneline);

  if (status === "unsafe") {
    blockers.push("status=unsafe");
  } else if (status !== "ready") {
    blockers.push(`status=${status} (not ready)`);
  }

  if (report.ready !== true) {
    blockers.push("ready=false");
  }

  const doctor =
    report.doctor && typeof report.doctor === "object"
      ? (report.doctor as { findings?: unknown })
      : null;
  const doctorFindings = Array.isArray(doctor?.findings) ? doctor.findings : [];
  if (
    doctorFindings.some(
      (finding) =>
        finding !== null &&
        typeof finding === "object" &&
        (finding as { code?: unknown }).code === "credential_migration_incomplete",
    )
  ) {
    blockers.push("credential migration reconciliation incomplete");
  }

  if (!parsed.healthy) {
    blockers.push(`pin unhealthy: ${parsed.kind} (${parsed.raw})`);
  }

  if (report.pin?.seal_ok === false) {
    blockers.push("pin.seal_ok=false");
  }
  if (report.pin?.expired === true) {
    blockers.push("pin.expired=true");
  }
  if (report.pin?.frozen === true) {
    blockers.push("pin.frozen=true");
  }

  if (requireContract) {
    const servers = parseRequiredServers(report.required_servers);
    if (!hasRequiredServers(servers)) {
      blockers.push(
        `required_servers missing locus and/or phantom (got: ${JSON.stringify(servers)})`,
      );
    }
    const cmd = (report.mcp_command ?? "").trim();
    if (cmd && cmd !== MCP_COMMAND) {
      blockers.push(`mcp_command must be ${MCP_COMMAND} (got: ${cmd})`);
    } else if (!cmd) {
      blockers.push(`mcp_command missing (expected ${MCP_COMMAND})`);
    }
  }

  // Deduplicate while preserving order
  return [...new Set(blockers)];
}

/**
 * Pure fleet gate evaluation from a parsed report.
 * `allowDispatch` is true only when blockers is empty.
 */
export function evaluateFleetGate(
  report: LocusAgentReport | null | undefined,
  opts?: EvaluateFleetGateOptions,
): Pick<LocusFleetGateResult, "allowDispatch" | "blockers" | "report" | "gateOk" | "status" | "status_oneline"> {
  const blockers = blockersFromAgentReport(report ?? null, opts);
  const status = report?.status;
  const status_oneline = report?.status_oneline;
  const gateOk =
    !!report &&
    report.ready === true &&
    report.status === "ready" &&
    isStatusOnelineHealthy(report.status_oneline ?? "");

  // allowDispatch requires no blockers (stricter than gateOk when contract checks on)
  const allowDispatch = blockers.length === 0;

  return {
    allowDispatch,
    blockers,
    report: report ?? null,
    gateOk,
    status,
    status_oneline,
  };
}

/**
 * Shell out to `locus agent report --json` and evaluate fleet pre-dispatch gate.
 *
 * Fail closed: missing CLI, empty/invalid report, status≠ready, unhealthy
 * oneline, or broken hub contract (required_servers / mcp_command).
 *
 * @see integrations/ashlr-hub/fleet-preflight.md
 * @see schema/hub-gate.schema.json
 */
export function locusFleetGate(env?: NodeJS.ProcessEnv): LocusFleetGateResult {
  const probe = locusAgentReport(env);
  if (!probe.available) {
    return {
      allowDispatch: false,
      blockers: [probe.error ?? "locus CLI not found on PATH"],
      report: null,
      available: false,
      gateOk: false,
    };
  }
  if (!probe.report) {
    return {
      allowDispatch: false,
      blockers: [probe.error ?? "locus agent report failed"],
      report: null,
      available: true,
      gateOk: false,
      status: undefined,
      status_oneline: undefined,
    };
  }

  const evaluated = evaluateFleetGate(probe.report);
  // Surface report findings as soft context only when already blocked
  if (!evaluated.allowDispatch && probe.report.next_steps?.length) {
    const hint = `next: ${probe.report.next_steps[0]}`;
    if (!evaluated.blockers.includes(hint)) {
      evaluated.blockers = [...evaluated.blockers, hint];
    }
  }

  return {
    ...evaluated,
    available: true,
  };
}

// ---------------------------------------------------------------------------
// Pre-mutate gate (opt-in enforce via LOCUS_ENFORCE and/or config.locus)
// ---------------------------------------------------------------------------

/**
 * Parse a single enforce token from env or config into a mode.
 *
 * | value | mode |
 * |-------|------|
 * | empty, 0, false, no, off | off |
 * | warn, log | warn |
 * | 1, true, yes, enforce, block | enforce |
 *
 * Unknown non-empty values fail closed to `enforce`.
 */
export function parseLocusEnforceToken(raw: unknown): LocusEnforceMode {
  if (raw === undefined || raw === null) return "off";
  const v = String(raw).trim().toLowerCase();
  if (!v || v === "0" || v === "false" || v === "no" || v === "off") {
    return "off";
  }
  if (v === "warn" || v === "log") return "warn";
  if (
    v === "1" ||
    v === "true" ||
    v === "yes" ||
    v === "enforce" ||
    v === "block"
  ) {
    return "enforce";
  }
  // Unknown token → enforce (fail closed; do not soft-allow typoed policy)
  return "enforce";
}

/**
 * Normalize a config input to the bare locus slice (`{ enforce?, firm? }`),
 * accepting AshlrConfig-shaped / `{ locus }` / bare locus objects.
 */
function locusSliceFromConfig(
  config?: LocusEnforceConfigInput,
): LocusFirmConfig | undefined {
  if (config == null || typeof config !== "object") return undefined;
  // AshlrConfig or { locus?: { ... } }
  if ("locus" in config) {
    const locus = (config as { locus?: unknown }).locus;
    if (locus != null && typeof locus === "object" && !Array.isArray(locus)) {
      return locus as LocusFirmConfig;
    }
    return undefined;
  }
  // Bare { enforce?, firm? } (or any object without a locus key)
  return config as LocusFirmConfig;
}

/**
 * Extract `locus.enforce` from a config slice / full AshlrConfig / bare
 * `{ enforce }` object. Returns undefined when the field is absent.
 */
export function extractLocusConfigEnforce(
  config?: LocusEnforceConfigInput,
): string | undefined {
  const slice = locusSliceFromConfig(config);
  if (
    slice == null ||
    slice.enforce === undefined ||
    slice.enforce === null
  ) {
    return undefined;
  }
  return String(slice.enforce);
}

/**
 * Extract `locus.firm` from a config slice / full AshlrConfig / bare
 * `{ firm }` object. Returns true only when the field is strictly boolean true.
 * Absent / false / non-boolean → false (monorepo-safe). Hub #258.
 */
export function extractLocusConfigFirm(
  config?: LocusEnforceConfigInput,
): boolean {
  const slice = locusSliceFromConfig(config);
  return slice?.firm === true;
}

/**
 * Best-effort read of `locus` from `~/.ashlr/config.json`.
 * Never throws; returns undefined on missing/unreadable config.
 * Used only by production wrappers (assert / session run) — pure resolvers
 * take an explicit config argument so unit tests stay hermetic.
 *
 * Hub production may re-wire this to `loadConfigReadOnly()`; the drop-in
 * stays standalone with a direct JSON read of the firm config file.
 */
export function readLocusConfigFromAshlr(): LocusFirmConfig | undefined {
  try {
    const path = join(homedir(), ".ashlr", "config.json");
    if (!existsSync(path)) return undefined;
    const raw = readFileSync(path, "utf8");
    const cfg = JSON.parse(raw) as { locus?: unknown };
    const locus = cfg?.locus;
    if (locus == null || typeof locus !== "object" || Array.isArray(locus)) {
      return undefined;
    }
    return locus as LocusFirmConfig;
  } catch {
    return undefined;
  }
}

/**
 * Resolve Locus enforce mode.
 *
 * Order (first match wins):
 *   1. env `LOCUS_ENFORCE` when the key is present (including empty → off)
 *   2. config `locus.enforce` when provided / present (explicit mode)
 *   3. config `locus.firm === true` → `enforce` (production fleet profile)
 *   4. `off` (monorepo-safe default — never always-on; firm defaults false)
 *
 * Pure when `config` is passed explicitly (or omitted for env-only). Does not
 * load ~/.ashlr itself; use {@link readLocusConfigFromAshlr} at call sites.
 *
 * Env always beats firm/enforce config (including LOCUS_ENFORCE=off). Explicit
 * `locus.enforce` beats `locus.firm`. Do not flip monorepo defaults to firm.
 */
export function resolveLocusEnforceMode(
  env?: NodeJS.ProcessEnv,
  config?: LocusEnforceConfigInput,
): LocusEnforceMode {
  const e = env ?? process.env;
  const rawEnv = e.LOCUS_ENFORCE;
  // Env wins whenever the key is set (including "" / "off" overriding config).
  if (rawEnv !== undefined && rawEnv !== null) {
    return parseLocusEnforceToken(rawEnv);
  }
  const rawCfg = extractLocusConfigEnforce(config);
  if (rawCfg !== undefined) {
    return parseLocusEnforceToken(rawCfg);
  }
  // Firm profile elevates only when enforce is still unset (not when
  // enforce is explicitly "off").
  if (extractLocusConfigFirm(config)) {
    return "enforce";
  }
  return "off";
}

/**
 * Pure: map a fleet-gate result + enforce mode into a pre-mutate decision.
 * Never throws. Never probes the CLI.
 */
export function decidePreMutateGate(
  gate: Pick<
    LocusFleetGateResult,
    "allowDispatch" | "blockers" | "gateOk" | "status" | "status_oneline" | "available"
  >,
  mode: LocusEnforceMode,
): LocusPreMutateDecision {
  if (mode === "off") {
    return {
      allow: true,
      mode,
      blockers: [],
      shouldWarn: false,
      gateOk: gate.gateOk,
      status: gate.status,
      status_oneline: gate.status_oneline,
      available: gate.available,
    };
  }

  const blockers = [...(gate.blockers ?? [])];
  const blocked = !gate.allowDispatch || blockers.length > 0;

  if (mode === "warn") {
    return {
      allow: true,
      mode,
      blockers: blocked ? blockers : [],
      shouldWarn: blocked,
      gateOk: gate.gateOk,
      status: gate.status,
      status_oneline: gate.status_oneline,
      available: gate.available,
    };
  }

  // enforce
  return {
    allow: !blocked,
    mode,
    blockers: blocked ? blockers : [],
    shouldWarn: false,
    gateOk: gate.gateOk,
    status: gate.status,
    status_oneline: gate.status_oneline,
    available: gate.available,
  };
}

/**
 * Pre-mutate gate for hub dispatch (fleet/run engines).
 *
 * - mode=off: returns allow without shelling out (monorepo-safe default).
 * - mode=warn|enforce: shells to `locusFleetGate` and decides.
 *
 * Mode resolution: env LOCUS_ENFORCE wins; else `config` / ~/.ashlr
 * `locus.enforce` / `locus.firm`; else off. Pass `config` explicitly in tests
 * to avoid reading the real home config (or pass `null` to disable config).
 *
 * Prefer this over bare `ensureLocusReady` at shared spawn sites so opt-in
 * enforcement does not break CI that lacks a Locus pin.
 */
export function assertLocusPreMutate(
  env?: NodeJS.ProcessEnv,
  config?: LocusEnforceConfigInput,
): LocusPreMutateDecision {
  // When the second arg is omitted, consult ~/.ashlr (firm profile path).
  // Explicit null/object skips the FS read so unit tests stay hermetic.
  const cfg = arguments.length >= 2 ? config : readLocusConfigFromAshlr();
  const mode = resolveLocusEnforceMode(env, cfg);
  if (mode === "off") {
    return {
      allow: true,
      mode,
      blockers: [],
      shouldWarn: false,
    };
  }
  const gate = locusFleetGate(env);
  return decidePreMutateGate(gate, mode);
}

/**
 * Format a pre-mutate decision for stderr / job UI (never includes secrets).
 */
export function formatPreMutateBlockers(decision: LocusPreMutateDecision): string {
  if (!decision.blockers.length) return "";
  return `locus pre-mutate ${decision.mode}: ${decision.blockers.join("; ")}`;
}

/**
 * Shared call-site helper: probe (when enforce mode is on via env or config),
 * log warn/block to stderr, return the decision. Callers refuse when
 * `!decision.allow`.
 *
 * Keeps spawnEngine / runSwarm / runApiModelSandboxed on one logging contract.
 * Never throws. Never logs secrets.
 *
 * When `config` is omitted, loads `locus` from ~/.ashlr/config.json (same as
 * {@link assertLocusPreMutate}). Pass `null` or an explicit object in tests.
 */
export function applyLocusPreMutateGate(
  env?: NodeJS.ProcessEnv,
  config?: LocusEnforceConfigInput,
): LocusPreMutateDecision {
  // Preserve "config omitted → load ~/.ashlr" vs "config null → no config".
  const decision =
    arguments.length >= 2
      ? assertLocusPreMutate(env, config)
      : assertLocusPreMutate(env);
  if (!decision.allow) {
    const msg =
      formatPreMutateBlockers(decision) ||
      "locus pre-mutate enforce: blocked (no detail)";
    try {
      process.stderr.write(`[locus] ${msg}\n`);
    } catch {
      // stderr may be closed in tests; ignore
    }
    return decision;
  }
  if (decision.shouldWarn) {
    const msg = formatPreMutateBlockers(decision);
    if (msg) {
      try {
        process.stderr.write(`[locus] ${msg}\n`);
      } catch {
        // ignore
      }
    }
  }
  return decision;
}

// ---------------------------------------------------------------------------
// CI / job session isolation (opt-in via LOCUS_CI_BINDING)
// ---------------------------------------------------------------------------

/**
 * Pure: decide how a mutating CI/job entry should obtain a Locus pin.
 *
 * | condition | kind |
 * |-----------|------|
 * | LOCUS_SESSION_ID set | already-session (skip re-mint) |
 * | LOCUS_CI_BINDING set | mint (prefer CI binding) |
 * | LOCUS_BINDING set | mint |
 * | enforce mode, no binding | refuse |
 * | warn mode, no binding | warn |
 * | otherwise | pass-through |
 *
 * Mode uses {@link resolveLocusEnforceMode} (env wins, then config, then off).
 * Never shells out. Never throws. Does not load ~/.ashlr — pass `config`
 * (or rely on {@link runWithLocusSessionIfConfigured} which loads it).
 */
export function decideLocusSessionRun(
  env?: NodeJS.ProcessEnv,
  config?: LocusEnforceConfigInput,
): LocusSessionRunDecision {
  const e = env ?? process.env;
  const sessionId = (e.LOCUS_SESSION_ID ?? "").trim();
  if (sessionId) {
    return { kind: "already-session", sessionId };
  }

  const ciBinding = (e.LOCUS_CI_BINDING ?? "").trim();
  if (ciBinding) {
    return {
      kind: "mint",
      binding: ciBinding,
      source: "LOCUS_CI_BINDING",
    };
  }

  const binding = (e.LOCUS_BINDING ?? "").trim();
  if (binding) {
    return { kind: "mint", binding, source: "LOCUS_BINDING" };
  }

  const mode = resolveLocusEnforceMode(e, config);
  if (mode === "enforce") {
    return {
      kind: "refuse",
      mode,
      reason:
        "LOCUS_ENFORCE requires LOCUS_CI_BINDING or LOCUS_BINDING for isolated CI sessions (no ambient ~/.locus pin)",
    };
  }
  if (mode === "warn") {
    return {
      kind: "warn",
      mode,
      reason:
        "LOCUS_ENFORCE=warn but LOCUS_CI_BINDING/LOCUS_BINDING unset — using ambient process env (may share ~/.locus pin)",
    };
  }
  return { kind: "pass-through", mode: "off" };
}

let callbackEnvTail: Promise<void> = Promise.resolve();

async function runWithScrubbedProcessEnv<T>(
  env: NodeJS.ProcessEnv,
  fn: () => Promise<T> | T,
): Promise<T> {
  let release!: () => void;
  const predecessor = callbackEnvTail;
  callbackEnvTail = new Promise<void>((resolveLock) => {
    release = resolveLock;
  });
  await predecessor;

  const original = { ...process.env };
  try {
    for (const key of Object.keys(process.env)) delete process.env[key];
    for (const [key, value] of Object.entries(env)) {
      if (typeof value === "string") process.env[key] = value;
    }
    delete process.env[CONTROL_CAPABILITY_ENV];
    return await fn();
  } finally {
    for (const key of Object.keys(process.env)) delete process.env[key];
    Object.assign(process.env, original);
    release();
  }
}

/**
 * Run `fn` under an ephemeral Locus CI session when configured.
 *
 * - Binding set (`LOCUS_CI_BINDING` or `LOCUS_BINDING`) → {@link withLocusSession}
 * - Enforce without binding → throws {@link LocusSessionConfigError}
 * - Warn without binding → logs and pass-through (`handle = null`)
 * - Otherwise → pass-through (monorepo-safe default)
 *
 * Prefer this at fleet/CI job entries so parallel agents do not share the
 * human shell pin (`~/.locus/active.json`). Callers that spawn children should
 * merge `handle.env` (via {@link applyLocusSessionEnv}) into child env.
 *
 * @example
 * ```ts
 * await runWithLocusSessionIfConfigured(async (handle) => {
 *   const env = { ...process.env };
 *   if (handle) applyLocusSessionEnv(env, handle.env);
 *   return spawnEngine(cmd, cfg, { env });
 * });
 * ```
 */
export async function runWithLocusSessionIfConfigured<T>(
  fn: (handle: LocusSessionHandle | null) => Promise<T> | T,
  opts?: RunWithLocusSessionOptions,
): Promise<T> {
  const env = opts?.env ?? process.env;
  // Consult ~/.ashlr locus.enforce / locus.firm when env LOCUS_ENFORCE is unset.
  const decision = decideLocusSessionRun(env, readLocusConfigFromAshlr());

  if (decision.kind === "mint") {
    return withLocusSession(
      decision.binding,
      (handle) => fn(handle),
      opts,
    );
  }

  if (decision.kind === "refuse") {
    throw new LocusSessionConfigError(decision.reason);
  }

  if (decision.kind === "already-session") {
    const handle = validateExistingLocusSession(env);
    return runWithScrubbedProcessEnv(handle.env, () => fn(handle));
  }

  if (decision.kind === "warn") {
    const msg = `[ashlr] locus session: ${decision.reason}`;
    if (opts?.onWarn) {
      opts.onWarn(msg);
    } else {
      try {
        process.stderr.write(`${msg}\n`);
      } catch {
        // stderr may be closed in tests
      }
    }
  }

  // pass-through | warn
  return await fn(null);
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
    const report = parseAgentReportJson(stdout);
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
 * Run `locus verify session --json` — full doctor + whoami + safe_next pack.
 * Preferred when hub needs structured session health (not just a compact tick).
 * Never throws. Never surfaces secret values (CLI contract).
 *
 * Note: CLI exits nonzero when `session_ok` is false but still emits JSON —
 * we parse stdout regardless of exit code.
 */
export function locusVerifySession(
  env?: NodeJS.ProcessEnv,
): LocusSessionVerifyResult {
  if (!locusAvailable()) {
    return {
      available: false,
      pack: null,
      exitCode: 2,
      error: "locus CLI not found on PATH",
      sessionOk: false,
    };
  }
  try {
    const r = spawnSync(LOCUS_BIN, ["verify", "session", "--json"], {
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
        pack: null,
        exitCode,
        error: r.stderr?.trim() || "empty verify session output",
        sessionOk: false,
      };
    }
    const pack = parseSessionVerificationPack(stdout);
    return {
      available: true,
      pack,
      exitCode,
      sessionOk: pack.session_ok === true,
    };
  } catch (e) {
    return {
      available: true,
      pack: null,
      exitCode: 2,
      error: e instanceof Error ? e.message : String(e),
      sessionOk: false,
    };
  }
}

/**
 * Run `locus watch --once --json` — single compact fleet heartbeat tick.
 * Prefer for continuous/periodic hub heartbeats; use
 * {@link locusVerifySession} when full doctor/whoami objects are needed.
 * Never throws. Never surfaces secret values.
 */
export function locusWatchOnce(env?: NodeJS.ProcessEnv): LocusWatchProbeResult {
  if (!locusAvailable()) {
    return {
      available: false,
      heartbeat: null,
      exitCode: 2,
      error: "locus CLI not found on PATH",
      ok: false,
    };
  }
  try {
    const r = spawnSync(LOCUS_BIN, ["watch", "--once", "--json"], {
      encoding: "utf8",
      timeout: TIMEOUT_MS,
      env: locusEnv(env),
      maxBuffer: 1024 * 1024,
    });
    const exitCode = typeof r.status === "number" ? r.status : 2;
    const stdout = (r.stdout ?? "").trim();
    if (!stdout) {
      return {
        available: true,
        heartbeat: null,
        exitCode,
        error: r.stderr?.trim() || "empty watch output",
        ok: false,
      };
    }
    const heartbeat = parseWatchHeartbeat(stdout);
    return {
      available: true,
      heartbeat,
      exitCode,
      ok: heartbeat.session_ok === true,
    };
  } catch (e) {
    return {
      available: true,
      heartbeat: null,
      exitCode: 2,
      error: e instanceof Error ? e.message : String(e),
      ok: false,
    };
  }
}

/**
 * Soft fleet-heartbeat probe for doctor/readiness under LOCUS_ENFORCE=warn.
 *
 * - mode=off / enforce: no shell-out from this helper (monorepo-safe default;
 *   enforce already has hard pre-mutate / readiness paths)
 * - mode=warn: runs `locus watch --once --json` for a soft annotation only
 * - Never throws; never hard-blocks by itself
 *
 * Pass `mode` explicitly in tests to stay hermetic (skips ~/.ashlr).
 */
export function locusSoftWatchHeartbeat(
  env?: NodeJS.ProcessEnv,
  mode?: LocusEnforceMode,
): LocusWatchProbeResult | null {
  const resolved =
    mode !== undefined
      ? mode
      : resolveLocusEnforceMode(env, readLocusConfigFromAshlr());
  // Soft roll-out only — never a second hard gate.
  if (resolved !== "warn") {
    return null;
  }
  return locusWatchOnce(env);
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
  if (mint.secrets_resolved) {
    throw new LocusMintError("ci mint unexpectedly returned resolved secrets");
  }
  mint.env = validateMintEnv(mint.env);
  return validateMintBinding(binding, mint);
}

/**
 * Run `fn` under an ephemeral Locus pin (`ci mint` → LOCUS_SESSION_ID).
 *
 * Use for hub job isolation so parallel agents do not share/mutate the human
 * shell pin (`active.json`). Parent credentials are scrubbed; the handle gets
 * runtime basics, caller-explicit env, and validated identity/scope metadata.
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
    ...scrubbedChildEnv(process.env, opts?.env),
    ...mint.env,
    LOCUS_HOME: home,
    LOCUS_SESSION_ID: mint.session_id,
    LOCUS_NOTIFY: "0",
    LOCUS_QUIET: "1",
  };

  const handle = validateExistingLocusSession(env, mint);
  return runWithScrubbedProcessEnv(handle.env, () => fn(handle));
}

// ---------------------------------------------------------------------------
// MCP config merge (pure + path helper)
// ---------------------------------------------------------------------------

/**
 * Build the locus MCP server entry (names/paths only — never secrets).
 */
export function locusServerSpec(opts?: MergeLocusMcpOptions): McpServerEntry {
  const home =
    opts?.locusHome ?? process.env.LOCUS_HOME ?? join(homedir(), ".locus");
  const env: Record<string, string> = {
    LOCUS_HOME: home,
    LOCUS_NOTIFY: "0",
    LOCUS_CLIENT: opts?.client ?? "ashlr-hub",
  };
  if (opts?.sessionId) {
    env.LOCUS_SESSION_ID = opts.sessionId;
  }
  return {
    command: opts?.command ?? MCP_COMMAND,
    args: [],
    env,
  };
}

/**
 * Parse MCP JSON text into an object. Empty/invalid → `{ mcpServers: {} }`.
 * Pure; never throws.
 */
export function parseMcpConfigJson(raw: string): McpConfigJson {
  const text = (raw ?? "").trim();
  if (!text) {
    return { mcpServers: {} };
  }
  try {
    const v = JSON.parse(text) as unknown;
    if (v === null || typeof v !== "object" || Array.isArray(v)) {
      return { mcpServers: {} };
    }
    return v as McpConfigJson;
  } catch {
    return { mcpServers: {} };
  }
}

function serverEntryEqual(a: unknown, b: McpServerEntry): boolean {
  try {
    return JSON.stringify(a) === JSON.stringify(b);
  } catch {
    return false;
  }
}

/**
 * Pure: merge locus server into an MCP config object under `mcpServers`.
 * Does not write the filesystem. Preserves other servers and top-level keys.
 */
export function mergeLocusIntoMcpConfig(
  config: McpConfigJson | Record<string, unknown> | null | undefined,
  opts?: MergeLocusMcpOptions,
): MergeLocusMcpResult {
  const serverName = opts?.name ?? "locus";
  const entry = locusServerSpec(opts);
  const base: McpConfigJson =
    config && typeof config === "object" && !Array.isArray(config)
      ? { ...(config as McpConfigJson) }
      : { mcpServers: {} };

  const existingServers =
    base.mcpServers &&
    typeof base.mcpServers === "object" &&
    !Array.isArray(base.mcpServers)
      ? { ...base.mcpServers }
      : {};

  const prev = existingServers[serverName];
  const changed = !serverEntryEqual(prev, entry);
  existingServers[serverName] = entry;

  return {
    config: { ...base, mcpServers: existingServers },
    changed,
    serverName,
  };
}

/**
 * Read MCP JSON at `path`, merge locus server, write if changed.
 * Creates parent directories. Safe for project `.mcp.json` / `.cursor/mcp.json`.
 *
 * @example
 * ```ts
 * registerLocusInMcpConfig(".mcp.json", { client: "ashlr-hub" });
 * ```
 */
export function registerLocusInMcpConfig(
  path: string,
  opts?: MergeLocusMcpOptions,
): RegisterLocusMcpResult {
  if (!path?.trim()) {
    throw new LocusMcpConfigError("path is required");
  }
  const created = !existsSync(path);
  let raw = "";
  if (!created) {
    try {
      raw = readFileSync(path, "utf8");
    } catch (e) {
      throw new LocusMcpConfigError(
        `failed to read ${path}: ${e instanceof Error ? e.message : String(e)}`,
      );
    }
  }
  const parsed = parseMcpConfigJson(raw);
  const merged = mergeLocusIntoMcpConfig(parsed, opts);

  if (!merged.changed && !created) {
    return {
      ...merged,
      path,
      written: false,
      created: false,
    };
  }

  try {
    const dir = dirname(path);
    if (dir && dir !== "." && !existsSync(dir)) {
      mkdirSync(dir, { recursive: true });
    }
    writeFileSync(path, `${JSON.stringify(merged.config, null, 2)}\n`, "utf8");
  } catch (e) {
    throw new LocusMcpConfigError(
      `failed to write ${path}: ${e instanceof Error ? e.message : String(e)}`,
    );
  }

  return {
    ...merged,
    path,
    written: true,
    created,
  };
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
  const locus = locusServerSpec({ locusHome: home });
  return {
    locus: {
      command: locus.command,
      args: locus.args ?? [],
      env: locus.env ?? {},
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

// ---------------------------------------------------------------------------
// Multi-tenant MCP grants (`locus mcp mint` → X-Locus-Tenant-Token dispatch)
// ---------------------------------------------------------------------------

/**
 * Header carrying the per-tenant grant token on EVERY multi-tenant `/mcp`
 * request (layered on the unchanged server bearer). Never a tool argument.
 */
export const TENANT_TOKEN_HEADER = "X-Locus-Tenant-Token" as const;

/** Default `locus-mcp --http` base URL (server DEFAULT_HTTP_ADDR). */
export const DEFAULT_MCP_HTTP_URL = "http://127.0.0.1:8742" as const;

/** `lmt_<grant_id>.<secret>` — charset-strict, mirrors locus-core's parser. */
const TENANT_TOKEN_RE = /^lmt_[a-f0-9]+\.[a-f0-9]+$/;

/** Lowercase-hex id charset shared by grant ids (`lmt_<grant_id>.…`). */
const GRANT_ID_RE = /^[a-f0-9]+$/;

/**
 * Output of `locus mcp mint --binding <alias> --json`.
 *
 * SECURITY: `token` is printed by the CLI exactly once (only its HMAC is
 * stored at rest under `$LOCUS_HOME/mcp-grants/`). Hubs must hold it in
 * memory only — never log, persist, or place it in tool arguments; it may
 * appear solely as the {@link TENANT_TOKEN_HEADER} header value.
 */
export interface LocusMcpGrantMint {
  grant_id: string;
  /** `lmt_<grant_id>.<secret>` bearer — memory only (see above). */
  token: string;
  session_id: string;
  binding: string;
  tenant: string;
  expires_at: string;
  label?: string | null;
  /** CLI serving hint (informational). */
  serve?: string;
}

/** One row of `locus mcp list --json` (values-free: never tokens). */
export interface LocusMcpGrantListEntry {
  grant_id: string;
  binding: string;
  tenant: string;
  label?: string | null;
  expires_at: string;
  expired: boolean;
  revoked: boolean;
  session_id: string;
  live_http_sessions: number;
}

/** Result of `locusMcpList` (never throws). */
export interface LocusMcpListResult {
  available: boolean;
  grants: LocusMcpGrantListEntry[];
  exitCode: number;
  error?: string;
}

/** Thrown on MT grant mint/parse/revoke failures (never carries a token). */
export class LocusMcpGrantError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "LocusMcpGrantError";
  }
}

/**
 * Pure: parse + validate `locus mcp mint --json` output.
 *
 * Fail closed: throws {@link LocusMcpGrantError} on invalid JSON, non-object
 * root, malformed token/session shapes, or a token that does not embed the
 * reported `grant_id` (a mint must never hand back someone else's grant).
 * Error messages never include the token.
 */
export function parseMcpMintOutput(raw: string): LocusMcpGrantMint {
  const text = (raw ?? "").trim();
  if (!text) {
    throw new LocusMcpGrantError("empty mcp mint JSON");
  }
  let v: unknown;
  try {
    v = JSON.parse(text);
  } catch {
    throw new LocusMcpGrantError("mcp mint output is not valid JSON");
  }
  if (v === null || typeof v !== "object" || Array.isArray(v)) {
    throw new LocusMcpGrantError("mcp mint root is not an object");
  }
  const obj = v as Record<string, unknown>;
  const grantId = typeof obj.grant_id === "string" ? obj.grant_id : "";
  const token = typeof obj.token === "string" ? obj.token : "";
  const sessionId = typeof obj.session_id === "string" ? obj.session_id : "";
  const binding = typeof obj.binding === "string" ? obj.binding : "";
  const tenant = typeof obj.tenant === "string" ? obj.tenant : "";
  const expiresAt = typeof obj.expires_at === "string" ? obj.expires_at : "";
  if (!GRANT_ID_RE.test(grantId)) {
    throw new LocusMcpGrantError("mcp mint grant_id is malformed");
  }
  if (!TENANT_TOKEN_RE.test(token) || !token.startsWith(`lmt_${grantId}.`)) {
    throw new LocusMcpGrantError("mcp mint token shape/grant binding is invalid");
  }
  if (!/^ses_[a-f0-9]+$/i.test(sessionId)) {
    throw new LocusMcpGrantError("mcp mint session_id is malformed");
  }
  if (!binding || !tenant || !expiresAt) {
    throw new LocusMcpGrantError("mcp mint is missing binding/tenant/expires_at");
  }
  return {
    grant_id: grantId,
    token,
    session_id: sessionId,
    binding,
    tenant,
    expires_at: expiresAt,
    label: typeof obj.label === "string" ? obj.label : null,
    serve: typeof obj.serve === "string" ? obj.serve : undefined,
  };
}

/**
 * Pure: parse + validate `locus mcp list --json` (operator roster).
 *
 * Fail closed: throws {@link LocusMcpGrantError} on invalid JSON, non-array
 * root, malformed rows, or ANY `lmt_` bearer material in the payload — the
 * roster is values-free by contract and a leaked token must never propagate.
 */
export function parseMcpListOutput(raw: string): LocusMcpGrantListEntry[] {
  const text = (raw ?? "").trim();
  if (!text) {
    throw new LocusMcpGrantError("empty mcp list JSON");
  }
  if (/lmt_[a-f0-9]+\.[a-f0-9]+/.test(text)) {
    throw new LocusMcpGrantError("mcp list payload contains bearer token material");
  }
  let v: unknown;
  try {
    v = JSON.parse(text);
  } catch {
    throw new LocusMcpGrantError("mcp list output is not valid JSON");
  }
  if (!Array.isArray(v)) {
    throw new LocusMcpGrantError("mcp list root is not an array");
  }
  return v.map((row, i) => {
    if (row === null || typeof row !== "object" || Array.isArray(row)) {
      throw new LocusMcpGrantError(`mcp list row ${i} is not an object`);
    }
    const r = row as Record<string, unknown>;
    const grantId = typeof r.grant_id === "string" ? r.grant_id : "";
    if (!GRANT_ID_RE.test(grantId)) {
      throw new LocusMcpGrantError(`mcp list row ${i} grant_id is malformed`);
    }
    if (typeof r.binding !== "string" || typeof r.tenant !== "string") {
      throw new LocusMcpGrantError(`mcp list row ${i} missing binding/tenant`);
    }
    if (typeof r.expired !== "boolean" || typeof r.revoked !== "boolean") {
      throw new LocusMcpGrantError(`mcp list row ${i} expired/revoked not boolean`);
    }
    const live = r.live_http_sessions;
    return {
      grant_id: grantId,
      binding: r.binding,
      tenant: r.tenant,
      label: typeof r.label === "string" ? r.label : null,
      expires_at: typeof r.expires_at === "string" ? r.expires_at : "",
      expired: r.expired,
      revoked: r.revoked,
      session_id: typeof r.session_id === "string" ? r.session_id : "",
      live_http_sessions:
        typeof live === "number" && Number.isFinite(live) && live >= 0 ? live : 0,
    };
  });
}

/** Typed reason for an MT `/mcp` auth failure. */
export type TenantAuthErrorKind =
  | "invalid_grant"
  | "grant_expired"
  | "tenant_mismatch"
  | "session_required"
  | "server_unauthorized"
  | "unknown";

/**
 * Classification of an MT `/mcp` HTTP failure into a typed reason + the ONE
 * safe recovery a hub may drive automatically.
 */
export interface TenantAuthClassification {
  kind: TenantAuthErrorKind;
  /**
   * - `re_mint`: operator mints a fresh grant (`locus mcp mint`) — covers
   *   invalid/revoked/expired tokens (revocation is never advertised, so
   *   invalid and revoked collapse together by design).
   * - `initialize`: POST `initialize` with YOUR tenant token to mint your own
   *   `Mcp-Session-Id` (cross-tenant reuse / missing session).
   * - `server_token`: fix the server bearer (`LOCUS_MCP_HTTP_TOKEN`).
   * - `none`: not a recognized MT auth failure — treat as hard error.
   */
  recovery: "re_mint" | "initialize" | "server_token" | "none";
  status: number;
  /** Verbatim `safe_next` command when the server offered one (expired). */
  safeNext?: string;
  /** Verbatim server hint (values-free by server contract). */
  hint?: string;
}

/** Best-effort object view of a response body (string JSON or object). */
function tenantErrorObject(body: unknown): Record<string, unknown> | null {
  if (typeof body === "string") {
    const text = body.trim();
    if (!text) return null;
    try {
      const v = JSON.parse(text) as unknown;
      if (v !== null && typeof v === "object" && !Array.isArray(v)) {
        return v as Record<string, unknown>;
      }
    } catch {
      return null;
    }
    return null;
  }
  if (body !== null && typeof body === "object" && !Array.isArray(body)) {
    return body as Record<string, unknown>;
  }
  return null;
}

/**
 * Pure: map an MT `/mcp` HTTP status + body onto a typed reason.
 *
 * | wire | kind | recovery |
 * |------|------|----------|
 * | 401 `{error:"invalid_grant",reason:"grant_expired"}` | grant_expired | re_mint |
 * | 401 `{error:"invalid_grant"}` (missing/forged/revoked) | invalid_grant | re_mint |
 * | 401 `{error:"unauthorized"}` (server bearer) | server_unauthorized | server_token |
 * | 403 `{error:"tenant_mismatch"}` | tenant_mismatch | initialize |
 * | 400 `{error:"session_required"}` | session_required | initialize |
 * | anything else | unknown | none |
 *
 * Never throws. Never echoes tokens (server bodies are values-free).
 */
export function classifyTenantAuthError(
  status: number,
  body: unknown,
): TenantAuthClassification {
  const obj = tenantErrorObject(body);
  const error = typeof obj?.error === "string" ? obj.error : "";
  const reason = typeof obj?.reason === "string" ? obj.reason : "";
  const hint = typeof obj?.hint === "string" ? obj.hint : undefined;
  const safeNext = typeof obj?.safe_next === "string" ? obj.safe_next : undefined;

  if (status === 401 && error === "invalid_grant") {
    if (reason === "grant_expired") {
      return { kind: "grant_expired", recovery: "re_mint", status, safeNext, hint };
    }
    return { kind: "invalid_grant", recovery: "re_mint", status, hint };
  }
  if (status === 401 && error === "unauthorized") {
    return { kind: "server_unauthorized", recovery: "server_token", status, hint };
  }
  if (status === 403 && error === "tenant_mismatch") {
    return { kind: "tenant_mismatch", recovery: "initialize", status, hint };
  }
  if (status === 400 && error === "session_required") {
    return { kind: "session_required", recovery: "initialize", status, hint };
  }
  return { kind: "unknown", recovery: "none", status, hint };
}

export interface WithLocusMcpTenantOptions {
  /** Grant TTL passed to `locus mcp mint` (CLI default 1h; cap ≤ job budget). */
  ttl?: string;
  /** Operator label shown by `locus mcp list` (use the hub job id). */
  label?: string;
  /** Allow bindings outside workspace allowlist. */
  force?: boolean;
  /** Server bearer (`LOCUS_MCP_HTTP_TOKEN`) merged into handle headers. */
  serverToken?: string;
  /** Extra env for the mint/revoke CLI spawns (e.g. LOCUS_HOME override). */
  env?: NodeJS.ProcessEnv;
  /** Override LOCUS_HOME for mint + revoke. */
  home?: string;
  /** Spawn timeout (ms). */
  timeoutMs?: number;
}

/**
 * Live handle for one tenant grant. `headers` carries the ONLY copy of the
 * bearer token; it is scrubbed when {@link withLocusMcpTenant} returns.
 */
export interface LocusMcpTenantHandle {
  grantId: string;
  binding: string;
  tenant: string;
  /** Sealed grant session id (values-free identity metadata). */
  sessionId: string;
  expiresAt: string;
  /**
   * Auth headers for every `/mcp` request: {@link TENANT_TOKEN_HEADER} plus
   * `Authorization: Bearer …` when `serverToken` was supplied. Memory only —
   * never log, persist, or copy out of the callback scope.
   */
  headers: Record<string, string>;
  /** Idempotent revoke (`locus mcp revoke <grant_id>`); also runs in finally. */
  revoke: () => void;
  /** True once the grant has been revoked. */
  revoked: boolean;
}

/**
 * Shell out to `locus mcp mint --binding <alias> --json` and validate.
 *
 * Fail closed: missing CLI, nonzero exit, malformed JSON, or a mint whose
 * `binding` differs from the request. The returned token is memory-only —
 * prefer {@link withLocusMcpTenant}, which scopes and scrubs it for you.
 */
export function locusMcpMint(
  binding: string,
  opts?: WithLocusMcpTenantOptions,
): LocusMcpGrantMint {
  if (!binding?.trim()) {
    throw new LocusMcpGrantError("binding alias is required");
  }
  if (!locusAvailable()) {
    throw new LocusMcpGrantError("locus CLI not found on PATH");
  }
  const args = ["mcp", "mint", "--binding", binding.trim(), "--json"];
  if (opts?.ttl) {
    args.push("--ttl", opts.ttl);
  }
  if (opts?.label) {
    args.push("--label", opts.label);
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
    maxBuffer: 1024 * 1024,
  });
  if (r.error) {
    throw new LocusMcpGrantError(`mcp mint failed: ${r.error.message}`);
  }
  if (r.status !== 0) {
    // stderr only — mint stdout is the one-time token payload, never echoed.
    throw new LocusMcpGrantError(
      `mcp mint exit ${r.status}: ${(r.stderr ?? "").trim() || "unknown"}`,
    );
  }
  const mint = parseMcpMintOutput(r.stdout ?? "");
  if (mint.binding !== binding.trim()) {
    throw new LocusMcpGrantError("mcp mint returned a different binding than requested");
  }
  return mint;
}

/**
 * Shell out to `locus mcp list --json` (operator-only roster; there is
 * deliberately NO HTTP enumeration). Never throws; values-free by contract.
 */
export function locusMcpList(env?: NodeJS.ProcessEnv): LocusMcpListResult {
  if (!locusAvailable()) {
    return {
      available: false,
      grants: [],
      exitCode: 2,
      error: "locus CLI not found on PATH",
    };
  }
  try {
    const r = spawnSync(LOCUS_BIN, ["mcp", "list", "--json"], {
      encoding: "utf8",
      timeout: TIMEOUT_MS,
      env: locusEnv(env),
      maxBuffer: 4 * 1024 * 1024,
    });
    const exitCode = typeof r.status === "number" ? r.status : 2;
    if (exitCode !== 0) {
      return {
        available: true,
        grants: [],
        exitCode,
        error: r.stderr?.trim() || "mcp list failed",
      };
    }
    return {
      available: true,
      grants: parseMcpListOutput(r.stdout ?? ""),
      exitCode,
    };
  } catch (e) {
    return {
      available: true,
      grants: [],
      exitCode: 2,
      error: e instanceof Error ? e.message : String(e),
    };
  }
}

/**
 * Shell out to `locus mcp revoke <grant_id>`.
 *
 * Throws {@link LocusMcpGrantError} on failure — a grant that outlives its
 * job is a live credential path, so callers must not swallow this silently.
 */
export function locusMcpRevoke(
  grantId: string,
  opts?: Pick<WithLocusMcpTenantOptions, "env" | "home" | "timeoutMs">,
): void {
  const id = (grantId ?? "").trim();
  if (!GRANT_ID_RE.test(id)) {
    throw new LocusMcpGrantError("grant id is malformed");
  }
  if (!locusAvailable()) {
    throw new LocusMcpGrantError("locus CLI not found on PATH");
  }
  const env = locusEnv({
    ...opts?.env,
    ...(opts?.home ? { LOCUS_HOME: opts.home } : {}),
  });
  const r = spawnSync(LOCUS_BIN, ["mcp", "revoke", id], {
    encoding: "utf8",
    timeout: opts?.timeoutMs ?? TIMEOUT_MS,
    env,
    maxBuffer: 1024 * 1024,
  });
  if (r.error) {
    throw new LocusMcpGrantError(`mcp revoke failed: ${r.error.message}`);
  }
  if (r.status !== 0) {
    throw new LocusMcpGrantError(
      `mcp revoke exit ${r.status}: ${(r.stderr ?? r.stdout ?? "").trim() || "unknown"}`,
    );
  }
}

/**
 * Run `fn` under a freshly minted multi-tenant MCP grant.
 *
 * Mints via `locus mcp mint --json`, hands `fn` a handle whose `headers`
 * carry the ONLY copy of the `lmt_` bearer for HTTP `/mcp` dispatch, and
 * ALWAYS revokes the grant in a finally (revocation propagates within one
 * request and sweeps the grant's `Mcp-Session-Id` records + worker homes).
 *
 * Token hygiene (hub contract):
 * - the token exists only in `handle.headers` for the callback's duration;
 * - it is scrubbed from the handle when this function returns, so a retained
 *   handle reference can never leak it;
 * - never log/persist it or place it in tool arguments.
 *
 * A revoke failure after a SUCCESSFUL callback is thrown (a leaked live
 * grant must not pass silently); after a failed callback the callback's
 * error wins and the revoke failure is noted on stderr (grant id only).
 *
 * @example
 * ```ts
 * await withLocusMcpTenant("cmp", async ({ headers, grantId }) => {
 *   const res = await fetch(`${url}/mcp`, {
 *     method: "POST",
 *     headers: { ...headers, "Content-Type": "application/json" },
 *     body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "initialize", params: {} }),
 *   });
 *   if (!res.ok) throw new Error(classifyTenantAuthError(res.status, await res.json()).kind);
 * }, { ttl: "15m", label: jobId, serverToken: process.env.LOCUS_MCP_HTTP_TOKEN });
 * ```
 */
export async function withLocusMcpTenant<T>(
  binding: string,
  fn: (handle: LocusMcpTenantHandle) => Promise<T> | T,
  opts?: WithLocusMcpTenantOptions,
): Promise<T> {
  const mint = locusMcpMint(binding, opts);
  const headers: Record<string, string> = {
    [TENANT_TOKEN_HEADER]: mint.token,
  };
  if (opts?.serverToken) {
    headers.Authorization = `Bearer ${opts.serverToken}`;
  }
  // Memory hygiene: the only live copy of the bearer now rides the handle.
  mint.token = "";
  const handle: LocusMcpTenantHandle = {
    grantId: mint.grant_id,
    binding: mint.binding,
    tenant: mint.tenant,
    sessionId: mint.session_id,
    expiresAt: mint.expires_at,
    headers,
    revoked: false,
    revoke: () => {
      if (handle.revoked) return;
      locusMcpRevoke(handle.grantId, opts);
      handle.revoked = true;
    },
  };
  let callbackFailed = false;
  try {
    return await fn(handle);
  } catch (e) {
    callbackFailed = true;
    throw e;
  } finally {
    try {
      handle.revoke();
    } catch (revokeError) {
      if (!callbackFailed) {
        throw revokeError;
      }
      // Callback error wins; still surface the leaked grant (id only).
      try {
        process.stderr.write(
          `[locus] mcp grant revoke failed for ${handle.grantId} — revoke manually\n`,
        );
      } catch {
        // stderr may be closed in tests
      }
    } finally {
      delete headers[TENANT_TOKEN_HEADER];
      delete headers.Authorization;
    }
  }
}

// ---------------------------------------------------------------------------
// MT preflight (HTTP — global fetch, no deps)
// ---------------------------------------------------------------------------

/** Minimal fetch surface so the drop-in stays dependency- and lib-agnostic. */
type FetchLike = (
  url: string,
  init?: { headers?: Record<string, string>; signal?: unknown },
) => Promise<{ ok: boolean; status: number; json(): Promise<unknown> }>;

export interface LocusMtPreflightOptions {
  /** Server base URL (default {@link DEFAULT_MCP_HTTP_URL}). */
  baseUrl?: string;
  /** Server bearer (`LOCUS_MCP_HTTP_TOKEN`) for the capabilities probe. */
  serverToken?: string;
  /**
   * Extra headers, e.g. an active {@link LocusMcpTenantHandle}'s `headers`
   * for a grant-authenticated probe (200 + `grant_id` instead of 401).
   */
  headers?: Record<string, string>;
  /** Per-request timeout (ms). */
  timeoutMs?: number;
  /** Injectable fetch for hermetic tests (default `globalThis.fetch`). */
  fetchFn?: FetchLike;
}

export interface LocusMtPreflightResult {
  /** True when GET /health answered 200. */
  reachable: boolean;
  /** True when the server demonstrably runs `--multi-tenant`. */
  multiTenant: boolean;
  /**
   * - `multi_tenant`: capabilities said `mode=multi_tenant`, OR /mcp answered
   *   401 `invalid_grant` (only the MT layer emits that body).
   * - `single_tenant`: capabilities 200 without the MT mode marker.
   * - `unknown`: unreachable / server bearer rejected / unrecognized.
   */
  mode: "multi_tenant" | "single_tenant" | "unknown";
  /** HTTP status of the /mcp probe when one was made. */
  status?: number;
  /** THIS grant's id when a tenant-authenticated probe succeeded. */
  grantId?: string | null;
  /** Typed classification when the /mcp probe failed auth. */
  auth?: TenantAuthClassification;
  error?: string;
}

/**
 * Probe a `locus-mcp --http` server: reachable (GET /health) + multi-tenant
 * mode detection (GET /mcp capabilities). Never throws; values-free.
 *
 * MT detection is sound both ways: with a tenant token the capabilities body
 * carries `mode: "multi_tenant"` + `grant_id`; without one, only an MT server
 * answers /mcp with 401 `invalid_grant` (single-tenant servers return 200
 * capabilities on a valid server bearer).
 */
export async function locusMtPreflight(
  opts?: LocusMtPreflightOptions,
): Promise<LocusMtPreflightResult> {
  const f =
    opts?.fetchFn ??
    ((globalThis as { fetch?: FetchLike }).fetch as FetchLike | undefined);
  if (!f) {
    return {
      reachable: false,
      multiTenant: false,
      mode: "unknown",
      error: "global fetch unavailable (Node >= 18 required)",
    };
  }
  const base = (opts?.baseUrl ?? DEFAULT_MCP_HTTP_URL).replace(/\/+$/, "");
  const timeoutMs = opts?.timeoutMs ?? TIMEOUT_MS;
  const signalOf = () =>
    (
      globalThis as {
        AbortSignal?: { timeout?: (ms: number) => unknown };
      }
    ).AbortSignal?.timeout?.(timeoutMs);

  try {
    const health = await f(`${base}/health`, { signal: signalOf() });
    if (!health.ok) {
      return {
        reachable: false,
        multiTenant: false,
        mode: "unknown",
        status: health.status,
        error: `GET /health status ${health.status}`,
      };
    }
  } catch (e) {
    return {
      reachable: false,
      multiTenant: false,
      mode: "unknown",
      error: e instanceof Error ? e.message : String(e),
    };
  }

  const headers: Record<string, string> = { ...opts?.headers };
  if (opts?.serverToken) {
    headers.Authorization = `Bearer ${opts.serverToken}`;
  }
  let status: number;
  let body: unknown = null;
  try {
    const caps = await f(`${base}/mcp`, { headers, signal: signalOf() });
    status = caps.status;
    try {
      body = await caps.json();
    } catch {
      body = null;
    }
  } catch (e) {
    return {
      reachable: true,
      multiTenant: false,
      mode: "unknown",
      error: e instanceof Error ? e.message : String(e),
    };
  }

  if (status === 200) {
    const obj = tenantErrorObject(body);
    if (obj?.mode === "multi_tenant") {
      return {
        reachable: true,
        multiTenant: true,
        mode: "multi_tenant",
        status,
        grantId: typeof obj.grant_id === "string" ? obj.grant_id : null,
      };
    }
    return { reachable: true, multiTenant: false, mode: "single_tenant", status };
  }

  const auth = classifyTenantAuthError(status, body);
  if (auth.kind === "invalid_grant" || auth.kind === "grant_expired") {
    // Only the MT grant layer emits invalid_grant on /mcp — mode proven.
    return {
      reachable: true,
      multiTenant: true,
      mode: "multi_tenant",
      status,
      auth,
      error: auth.kind,
    };
  }
  return {
    reachable: true,
    multiTenant: false,
    mode: "unknown",
    status,
    auth,
    error: auth.kind,
  };
}
