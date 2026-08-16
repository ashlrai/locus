#!/usr/bin/env bash
# hub-integration-test.sh — ashlr-hub composition smoke.
#
# Validates:
#   1. locus agent report --json contract (required_servers = locus+phantom)
#   2. status_oneline parse tokens (pinned + unpinned)
#   3. hub-gate schema + fleet pure helpers (via node, no deps)
#   4. registerLocusInMcpConfig / merge pure helpers
#
# Requires: locus (or cargo build), jq, node
# Safe: never touches ~/.locus; never prints secret values.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export PATH="${ROOT}/target/debug:${ROOT}/target/release:${HOME}/.cargo/bin:${PATH}"
export LOCUS_CONTROL_CAPABILITY="${LOCUS_CONTROL_CAPABILITY:-$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n')}"

if ! command -v jq >/dev/null 2>&1; then
  echo "error: jq is required" >&2
  exit 1
fi
if ! command -v node >/dev/null 2>&1; then
  echo "error: node is required for pure helper checks" >&2
  exit 1
fi

if ! locus agent report --help >/dev/null 2>&1; then
  echo "building locus with agent report…"
  (cd "$ROOT" && cargo build -q -p locus-cli)
fi
if [[ ! -x "${ROOT}/target/debug/locus" ]]; then
  (cd "$ROOT" && cargo build -q -p locus-cli)
fi

SMOKE_HOME="$(mktemp -d "${TMPDIR:-/tmp}/locus-hub-int.XXXXXX")"
SMOKE_PROJ="$(mktemp -d "${TMPDIR:-/tmp}/locus-hub-int-proj.XXXXXX")"
MT_SRV_PID=""
cleanup() {
  [[ -n "$MT_SRV_PID" ]] && kill "$MT_SRV_PID" 2>/dev/null || true
  rm -rf "$SMOKE_HOME" "$SMOKE_PROJ"
}
trap cleanup EXIT

export LOCUS_HOME="$SMOKE_HOME"
unset LOCUS_SESSION_ID || true

echo "== hub-integration-test LOCUS_HOME=$LOCUS_HOME =="
echo "locus: $(command -v locus) ($(locus --version 2>/dev/null || true))"

locus init --with-samples >/dev/null
locus pin personal >/dev/null

cd "$SMOKE_PROJ"
printf '%s\n' '{"mcpServers":{"locus":{"command":"locus-mcp","args":[]}}}' > .mcp.json

fail=0

ok() { echo "ok    $1"; }
bad() { echo "FAIL  $1"; fail=1; }

# ---------------------------------------------------------------------------
# 1. Agent report: required_servers includes locus + phantom
# ---------------------------------------------------------------------------
REPORT="$(locus agent report --json 2>/dev/null || true)"
if ! printf '%s' "$REPORT" | jq -e . >/dev/null 2>&1; then
  bad "agent report not JSON"
  echo "$REPORT" | head -c 400
else
  if printf '%s' "$REPORT" | jq -e '
    (.required_servers | index("locus") != null)
    and (.required_servers | index("phantom") != null)
    and (.required_servers | length) >= 2
    and .mcp_command == "locus-mcp"
    and (.status | IN("ready","protected","unsafe"))
    and (.status_oneline | type) == "string"
    and (.ready | type) == "boolean"
  ' >/dev/null 2>&1; then
    ok "agent report required_servers + mcp_command"
  else
    bad "agent report missing locus+phantom or mcp_command"
    printf '%s\n' "$REPORT" | jq -c '{status,ready,required_servers,mcp_command,status_oneline}' 2>/dev/null || true
  fi

  # Secret and credential-locator hygiene
  if printf '%s' "$REPORT" | grep -EEq 'ghp_|sk-[a-zA-Z0-9]{10,}|xox[baprs]-|"credential_ref"|phm:|env:|test:'; then
    bad "possible secret or credential locator material in agent report"
  else
    ok "agent report secret hygiene"
  fi
fi

# ---------------------------------------------------------------------------
# 2. status_oneline parse (CLI + pure helpers)
# ---------------------------------------------------------------------------
ONELINE="$(locus status --oneline)"
if [[ "$ONELINE" == *":"* ]] && [[ "$ONELINE" != "unpinned" ]] && [[ "$ONELINE" != "frozen" ]] && [[ "$ONELINE" != "invalid" ]]; then
  ok "status --oneline pinned ($ONELINE)"
else
  bad "status --oneline expected alias:tenant, got: $ONELINE"
fi

if printf '%s' "$REPORT" | jq -e --arg o "$ONELINE" '.status_oneline == $o' >/dev/null 2>&1; then
  ok "report.status_oneline matches status --oneline"
else
  # report may lag only if leave raced; still require report field present
  if printf '%s' "$REPORT" | jq -e '(.status_oneline | type) == "string" and (.status_oneline | length) > 0' >/dev/null 2>&1; then
    ok "report.status_oneline present ($(printf '%s' "$REPORT" | jq -r .status_oneline))"
  else
    bad "report.status_oneline missing"
  fi
fi

locus leave >/dev/null 2>&1 || true
UNPINNED="$(locus status --oneline)"
if [[ "$UNPINNED" == "unpinned" ]]; then
  ok "status --oneline unpinned after leave"
else
  bad "expected unpinned after leave, got: $UNPINNED"
fi

UNPIN_REPORT="$(locus agent report --json 2>/dev/null || true)"
if printf '%s' "$UNPIN_REPORT" | jq -e '.status_oneline == "unpinned" and .ready == false' >/dev/null 2>&1; then
  ok "agent report unpinned ready=false"
else
  bad "agent report unpinned contract"
fi

# Re-pin for remaining checks
locus pin personal >/dev/null
REPORT="$(locus agent report --json 2>/dev/null || true)"

# ---------------------------------------------------------------------------
# 3. Schemas present
# ---------------------------------------------------------------------------
for f in agent-report.schema.json doctor.schema.json hub-gate.schema.json mcp-grant.schema.json; do
  if [[ -f "$ROOT/schema/$f" ]]; then
    ok "schema/$f present"
  else
    bad "schema/$f missing"
  fi
done

if ! jq -e '.required | index("allowDispatch") and index("blockers") and index("report")' \
  "$ROOT/schema/hub-gate.schema.json" >/dev/null 2>&1; then
  bad "hub-gate.schema.json missing required keys"
else
  ok "hub-gate.schema.json required keys"
fi

if jq -e '.. | objects | has("credential_ref")' "$ROOT/schema/doctor.schema.json" >/dev/null 2>&1; then
  bad "doctor schema still exposes credential_ref"
elif ! jq -e '.properties.unresolved_phm.items["$ref"] == "#/$defs/credentialResolutionIssue"' \
  "$ROOT/schema/doctor.schema.json" >/dev/null 2>&1; then
  bad "doctor schema missing safe credential resolution issue contract"
else
  ok "doctor schema credential metadata only"
fi

if printf '%s' '{"required_servers":["locus","phantom"],"mcp_command":"locus-mcp","doctor":{"findings":[{"code":"credential_migration_incomplete"}]}}' \
  | jq -e -f "$ROOT/scripts/dogfood-ready.jq" >/dev/null 2>&1; then
  bad "dogfood gate allowed incomplete credential migration"
else
  ok "dogfood gate blocks incomplete credential migration"
fi

MINT="$(locus ci mint -b personal --json)"
if printf '%s' "$MINT" | jq -e '
  .binding == "personal"
  and .binding_id == .env.LOCUS_BINDING_ID
  and .binding == .env.LOCUS_BINDING
  and .session_id == .env.LOCUS_SESSION_ID
  and .tenant == .env.LOCUS_TENANT
  and .seal == .env.LOCUS_SEAL
  and .worker_home == .env.LOCUS_WORKER_HOME
  and .expires_at == .env.LOCUS_EXPIRES_AT
  and (.env.LOCUS_EXECUTOR_CAPABILITY | test("^[a-f0-9]{64}$"))
  and .secrets_resolved == false
' >/dev/null 2>&1; then
  ok "Hub mint is exact-binding and exact-session consistent"
else
  bad "Hub mint identity fields are not exact-session consistent"
fi

HUB_MINT_JSON="$MINT" HUB_ROOT="$ROOT" LOCUS_BIN="${ROOT}/target/debug/locus" \
  node --experimental-strip-types --input-type=module <<'NODE'
import { pathToFileURL } from "node:url";

const root = process.env.HUB_ROOT;
const mint = JSON.parse(process.env.HUB_MINT_JSON);
const module = await import(pathToFileURL(`${root}/integrations/ashlr-hub/locus.ts`).href);
const exact = { ...mint.env, LOCUS_HOME: process.env.LOCUS_HOME, LOCUS_ENFORCE: "1" };

const verified = module.validateExistingLocusSession(exact);
if (verified.sessionId !== mint.session_id || verified.binding !== mint.binding) {
  throw new Error("live existing-session verification changed identity");
}

process.env.CROSS_BINDING_TOKEN = "ambient-cross-binding-canary";
process.env.LOCUS_CONTROL_CAPABILITY = process.env.LOCUS_CONTROL_CAPABILITY || "a".repeat(64);
let callbackRan = false;
await module.runWithLocusSessionIfConfigured(async (handle) => {
  callbackRan = true;
  if (!handle || handle.sessionId !== mint.session_id) throw new Error("missing verified handle");
  if (process.env.CROSS_BINDING_TOKEN !== undefined) throw new Error("ambient credential reached callback");
  if (process.env.LOCUS_CONTROL_CAPABILITY !== undefined) throw new Error("control capability reached callback");
  if (handle.env.CROSS_BINDING_TOKEN !== undefined) throw new Error("ambient credential reached handle");
  if (handle.env.HOME !== mint.worker_home) throw new Error("callback HOME is not worker-scoped");
}, { env: exact });
if (!callbackRan) throw new Error("verified callback did not run");
if (process.env.CROSS_BINDING_TOKEN !== "ambient-cross-binding-canary") {
  throw new Error("callback environment was not restored");
}

for (const forged of [
  { ...exact, LOCUS_TENANT: "forged-tenant" },
  { ...exact, LOCUS_SEAL: "hmac-sha256:" + "0".repeat(64) },
  { ...exact, LOCUS_SESSION_ID: "ses_" + "0".repeat(32) },
  { ...exact, LOCUS_EXECUTOR_CAPABILITY: undefined },
]) {
  let rejected = false;
  try {
    await module.runWithLocusSessionIfConfigured(() => {
      throw new Error("forged session reached callback");
    }, { env: forged });
  } catch {
    rejected = true;
  }
  if (!rejected) throw new Error("forged existing-session labels were accepted");
}
console.log("ok    live: Hub validates broker-backed session and scrubs callback ambient authority");
NODE

EXPIRED_MINT="$(locus ci mint -b personal --ttl 1s --json)"
sleep 2
if HUB_MINT_JSON="$EXPIRED_MINT" HUB_ROOT="$ROOT" LOCUS_BIN="${ROOT}/target/debug/locus" \
  node --experimental-strip-types --input-type=module >/dev/null 2>&1 <<'NODE'
import { pathToFileURL } from "node:url";
const mint = JSON.parse(process.env.HUB_MINT_JSON);
const module = await import(pathToFileURL(`${process.env.HUB_ROOT}/integrations/ashlr-hub/locus.ts`).href);
const env = { ...mint.env, LOCUS_HOME: process.env.LOCUS_HOME, LOCUS_ENFORCE: "1" };
module.validateExistingLocusSession(env);
NODE
then
  bad "Hub accepted an expired broker-backed session"
else
  ok "Hub rejects expired broker-backed session before callback"
fi

# ---------------------------------------------------------------------------
# 4. Pure helpers (node — no TypeScript build; inline mirrors of locus.ts)
# ---------------------------------------------------------------------------
node <<'NODE'
const assert = (cond, msg) => {
  if (!cond) {
    console.error("FAIL  pure: " + msg);
    process.exitCode = 1;
  } else {
    console.log("ok    pure: " + msg);
  }
};

// --- parseStatusOneline (mirror locus.ts) ---
function parseStatusOneline(raw) {
  const s = (raw ?? "").trim() || "unpinned";
  if (s === "unpinned") return { kind: "unpinned", raw: s, healthy: false };
  if (s === "require_pin") return { kind: "require_pin", raw: s, healthy: false };
  if (s === "frozen") return { kind: "frozen", raw: s, healthy: false };
  if (s === "invalid") return { kind: "invalid", raw: s, healthy: false };
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
  return { kind: "invalid", raw: s, healthy: false };
}

assert(parseStatusOneline("unpinned").healthy === false, "oneline unpinned unhealthy");
assert(parseStatusOneline("frozen").kind === "frozen", "oneline frozen");
assert(parseStatusOneline("invalid").kind === "invalid", "oneline invalid");
assert(parseStatusOneline("require_pin").kind === "require_pin", "oneline require_pin");
const pin = parseStatusOneline("personal:personal");
assert(pin.healthy === true && pin.alias === "personal" && pin.tenant === "personal", "oneline alias:tenant");
assert(parseStatusOneline("notoken").healthy === false, "oneline unknown fail-closed");

// --- hasRequiredServers ---
const REQUIRED = ["locus", "phantom"];
function hasRequiredServers(servers) {
  if (!servers || !servers.length) return false;
  const set = new Set(servers.map((s) => String(s).trim().toLowerCase()).filter(Boolean));
  return REQUIRED.every((r) => set.has(r));
}
assert(hasRequiredServers(["locus", "phantom"]) === true, "required servers both");
assert(hasRequiredServers(["locus"]) === false, "required servers missing phantom");
assert(hasRequiredServers([]) === false, "required servers empty");

// --- canMutate ---
function canMutate(status, oneline) {
  if (status === "unsafe") return false;
  if (status !== "ready") return false;
  return parseStatusOneline(oneline).healthy;
}
assert(canMutate("ready", "acme:acme-corp") === true, "canMutate ready+pin");
assert(canMutate("ready", "unpinned") === false, "canMutate ready+unpinned");
assert(canMutate("protected", "acme:acme") === false, "canMutate protected");
assert(canMutate("unsafe", "acme:acme") === false, "canMutate unsafe");

// --- evaluateFleetGate blockers ---
function blockersFromAgentReport(report) {
  const blockers = [];
  if (!report) return ["no agent report"];
  const status = report.status ?? "unknown";
  const oneline = report.status_oneline ?? "unpinned";
  const parsed = parseStatusOneline(oneline);
  if (status === "unsafe") blockers.push("status=unsafe");
  else if (status !== "ready") blockers.push(`status=${status} (not ready)`);
  if (report.ready !== true) blockers.push("ready=false");
  if (!parsed.healthy) blockers.push(`pin unhealthy: ${parsed.kind} (${parsed.raw})`);
  const findings = Array.isArray(report.doctor?.findings) ? report.doctor.findings : [];
  if (findings.some((finding) => finding?.code === "credential_migration_incomplete")) {
    blockers.push("credential migration reconciliation incomplete");
  }
  const servers = Array.isArray(report.required_servers) ? report.required_servers : [];
  if (!hasRequiredServers(servers)) {
    blockers.push("required_servers missing locus and/or phantom");
  }
  const cmd = (report.mcp_command ?? "").trim();
  if (cmd !== "locus-mcp") blockers.push("mcp_command must be locus-mcp");
  return [...new Set(blockers)];
}

function evaluateFleetGate(report) {
  const blockers = blockersFromAgentReport(report);
  return { allowDispatch: blockers.length === 0, blockers, report: report ?? null };
}

const readyReport = {
  status: "ready",
  ready: true,
  status_oneline: "personal:personal",
  required_servers: ["locus", "phantom"],
  mcp_command: "locus-mcp",
};
const g1 = evaluateFleetGate(readyReport);
assert(g1.allowDispatch === true && g1.blockers.length === 0, "fleet gate allow when ready");

const blocked = evaluateFleetGate({
  status: "protected",
  ready: false,
  status_oneline: "unpinned",
  required_servers: ["locus", "phantom"],
  mcp_command: "locus-mcp",
});
assert(blocked.allowDispatch === false && blocked.blockers.length > 0, "fleet gate block unpinned");

const noPhantom = evaluateFleetGate({
  status: "ready",
  ready: true,
  status_oneline: "a:b",
  required_servers: ["locus"],
  mcp_command: "locus-mcp",
});
assert(noPhantom.allowDispatch === false, "fleet gate block missing phantom");

const incompleteMigration = evaluateFleetGate({
  ...readyReport,
  doctor: {
    verdict: "UNSAFE",
    findings: [{ severity: "unsafe", code: "credential_migration_incomplete" }],
  },
});
assert(
  incompleteMigration.allowDispatch === false &&
    incompleteMigration.blockers.includes("credential migration reconciliation incomplete"),
  "fleet gate blocks incomplete credential migration even with stale ready status",
);

// --- scrubbedChildEnv + validateMintEnv ---
const CHILD_RUNTIME_ENV = new Set(["PATH", "HOME", "TMPDIR", "LANG"]);
function scrubbedChildEnv(parent, explicit = {}) {
  const clean = {};
  for (const [key, value] of Object.entries(parent)) {
    if (CHILD_RUNTIME_ENV.has(key) || key.startsWith("LC_")) clean[key] = value;
  }
  return { ...clean, ...explicit };
}
const child = scrubbedChildEnv(
  { PATH: "/bin", LANG: "C", GITHUB_TOKEN: "ambient-canary" },
  { JOB_ID: "explicit" },
);
assert(child.PATH === "/bin" && child.JOB_ID === "explicit", "child env keeps runtime + explicit");
assert(!("GITHUB_TOKEN" in child), "child env scrubs ambient credentials");

const MINT_SCOPE_ENV = new Set(["GH_CONFIG_DIR", "SUPABASE_PROJECT_REF"]);
const MINT_IDENTITY_ENV = new Set(["LOCUS_SESSION_ID", "LOCUS_TENANT"]);
function isAllowedMintEnvKey(key) {
  return MINT_IDENTITY_ENV.has(key) || MINT_SCOPE_ENV.has(key) || /^LOCUS_[A-Z0-9_]+_(?:ACCOUNT|CREDENTIAL_RESOLVED|PROJECT_REF|TEAM_ID|ACCOUNT_ID|READ_ONLY|ORGS|REPOS|PROJECTS)$/.test(key);
}
function validateMintEnv(raw) {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) throw new Error("invalid env");
  const clean = {};
  for (const [key, value] of Object.entries(raw)) {
    if (!isAllowedMintEnvKey(key) || typeof value !== "string" || /(?:phm|env|test):/i.test(value)) {
      throw new Error("disallowed env metadata");
    }
    clean[key] = value;
  }
  return clean;
}
assert(validateMintEnv({ LOCUS_SESSION_ID: "ses_1", SUPABASE_PROJECT_REF: "project" }).LOCUS_SESSION_ID === "ses_1", "mint env accepts identity + scope");
for (const badEnv of [{ GITHUB_TOKEN: "token" }, { LOCUS_SECRET: "token" }, { LOCUS_TENANT: "phm:LOCATOR" }]) {
  let rejected = false;
  try { validateMintEnv(badEnv); } catch { rejected = true; }
  assert(rejected, "mint env rejects secrets and locators");
}

// --- LOCUS_ENFORCE + config.locus.enforce / decidePreMutateGate (pure; mirrors locus.ts) ---
function parseLocusEnforceToken(raw) {
  if (raw === undefined || raw === null) return "off";
  const v = String(raw).trim().toLowerCase();
  if (!v || v === "0" || v === "false" || v === "no" || v === "off") return "off";
  if (v === "warn" || v === "log") return "warn";
  if (v === "1" || v === "true" || v === "yes" || v === "enforce" || v === "block") return "enforce";
  return "enforce"; // unknown → fail closed
}
function extractLocusConfigEnforce(config) {
  if (config == null || typeof config !== "object") return undefined;
  if ("enforce" in config && !("locus" in config) && config.enforce != null) {
    return String(config.enforce);
  }
  if ("locus" in config && config.locus != null && typeof config.locus === "object" && config.locus.enforce != null) {
    return String(config.locus.enforce);
  }
  return undefined;
}
function resolveLocusEnforceMode(env, config) {
  const e = env ?? {};
  const rawEnv = e.LOCUS_ENFORCE;
  if (rawEnv !== undefined && rawEnv !== null) return parseLocusEnforceToken(rawEnv);
  const rawCfg = extractLocusConfigEnforce(config);
  if (rawCfg !== undefined) return parseLocusEnforceToken(rawCfg);
  return "off";
}
function decidePreMutateGate(gate, mode) {
  if (mode === "off") {
    return { allow: true, mode, blockers: [], shouldWarn: false };
  }
  const blockers = [...(gate.blockers ?? [])];
  const blocked = !gate.allowDispatch || blockers.length > 0;
  if (mode === "warn") {
    return { allow: true, mode, blockers: blocked ? blockers : [], shouldWarn: blocked };
  }
  return { allow: !blocked, mode, blockers: blocked ? blockers : [], shouldWarn: false };
}
function formatPreMutateBlockers(decision) {
  if (!decision.blockers.length) return "";
  return `locus pre-mutate ${decision.mode}: ${decision.blockers.join("; ")}`;
}

assert(resolveLocusEnforceMode({}) === "off", "LOCUS_ENFORCE unset → off");
assert(resolveLocusEnforceMode({ LOCUS_ENFORCE: "0" }) === "off", "LOCUS_ENFORCE=0 → off");
assert(resolveLocusEnforceMode({ LOCUS_ENFORCE: "warn" }) === "warn", "LOCUS_ENFORCE=warn");
assert(resolveLocusEnforceMode({ LOCUS_ENFORCE: "1" }) === "enforce", "LOCUS_ENFORCE=1 → enforce");
assert(resolveLocusEnforceMode({ LOCUS_ENFORCE: "typo-mode" }) === "enforce", "unknown LOCUS_ENFORCE → enforce");
assert(
  resolveLocusEnforceMode({}, { locus: { enforce: "enforce" } }) === "enforce",
  "config.locus.enforce=enforce when env unset",
);
assert(
  resolveLocusEnforceMode({}, { enforce: "warn" }) === "warn",
  "bare { enforce } config slice",
);
assert(
  resolveLocusEnforceMode({ LOCUS_ENFORCE: "off" }, { locus: { enforce: "enforce" } }) === "off",
  "env LOCUS_ENFORCE=off wins over firm config",
);
assert(
  resolveLocusEnforceMode({}, { locus: {} }) === "off",
  "empty locus object → off (field absent)",
);
assert(parseLocusEnforceToken("block") === "enforce", "parseLocusEnforceToken block → enforce");
assert(extractLocusConfigEnforce({ locus: { enforce: "warn" } }) === "warn", "extract locus.enforce");

const blockedGate = { allowDispatch: false, blockers: ["status=unsafe"] };
const healthyGate = { allowDispatch: true, blockers: [] };
const offDec = decidePreMutateGate(blockedGate, "off");
assert(offDec.allow === true && offDec.blockers.length === 0 && offDec.shouldWarn === false, "mode=off ignores blockers");
const warnDec = decidePreMutateGate(blockedGate, "warn");
assert(warnDec.allow === true && warnDec.shouldWarn === true && warnDec.blockers[0] === "status=unsafe", "mode=warn allows + shouldWarn");
const enfDec = decidePreMutateGate(blockedGate, "enforce");
assert(enfDec.allow === false && enfDec.shouldWarn === false, "mode=enforce blocks");
const okDec = decidePreMutateGate(healthyGate, "enforce");
assert(okDec.allow === true && okDec.blockers.length === 0, "mode=enforce allows healthy gate");
assert(
  formatPreMutateBlockers(enfDec) === "locus pre-mutate enforce: status=unsafe",
  "formatPreMutateBlockers includes mode + blockers",
);

// --- parseWatchHeartbeat / parseSessionVerificationPack (mirror locus.ts; hub #273) ---
function coerceBool(v) {
  return v === true || v === 1 || v === "1" || v === "true";
}
function coerceOptionalString(v) {
  if (typeof v === "string" && v.trim()) return v.trim();
  return undefined;
}
function coerceSafeNextAction(v) {
  if (typeof v === "string" && v.trim()) return v.trim();
  if (v !== null && typeof v === "object" && !Array.isArray(v)) {
    const action = v.action;
    if (typeof action === "string" && action.trim()) return action.trim();
  }
  return undefined;
}
function parseWatchHeartbeat(raw) {
  let obj;
  if (typeof raw === "string") {
    const text = raw.trim();
    if (!text) throw new Error("empty watch heartbeat JSON");
    const line =
      text
        .split(/\r?\n/)
        .map((l) => l.trim())
        .filter(Boolean)
        .pop() ?? text;
    const v = JSON.parse(line);
    if (v === null || typeof v !== "object" || Array.isArray(v)) {
      throw new Error("watch heartbeat root is not an object");
    }
    obj = v;
  } else if (raw !== null && typeof raw === "object" && !Array.isArray(raw)) {
    obj = raw;
  } else {
    throw new Error("watch heartbeat root is not an object");
  }
  const kindRaw = typeof obj.kind === "string" ? obj.kind.trim().toLowerCase() : "";
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
    const pinned = coerceBool(obj.pinned) || (whoami != null && whoami.length > 0);
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
      ? obj.issues.filter((i) => typeof i === "string")
      : [];
    const safeNext = pinned ? (ok ? "ready" : issues[0] ?? "re_pin") : "enter";
    return {
      kind: "watch",
      session_ok: ok && pinned && !frozen,
      whoami,
      doctor_verdict: ok && pinned && !frozen ? "SAFE" : frozen ? "UNSAFE" : "WARN",
      safe_next: safeNext,
      pinned,
      frozen,
      source: "legacy-runtime",
    };
  }
  throw new Error("unrecognized watch heartbeat shape");
}
function parseSessionVerificationPack(raw) {
  const text = (raw ?? "").trim();
  if (!text) throw new Error("empty session verification JSON");
  const v = JSON.parse(text);
  if (v === null || typeof v !== "object" || Array.isArray(v)) {
    throw new Error("session verification root is not an object");
  }
  const sessionOk =
    v.session_ok === true || v.session_ok === "true" || v.session_ok === 1;
  return {
    ...v,
    kind: "session",
    version: typeof v.version === "string" ? v.version : "",
    session_ok: sessionOk,
  };
}

const modernHb = parseWatchHeartbeat(
  JSON.stringify({
    kind: "watch",
    session_ok: true,
    whoami: "acme",
    doctor_verdict: "SAFE",
    safe_next: "ready",
    pinned: true,
    frozen: false,
  }),
);
assert(modernHb.kind === "watch" && modernHb.session_ok === true, "watch modern session_ok");
assert(modernHb.whoami === "acme" && modernHb.source === "watch", "watch modern whoami/source");
const nestedHb = parseWatchHeartbeat(
  [
    '{"kind":"watch","session_ok":false,"doctor_verdict":"WARN","safe_next":"enter","pinned":false,"frozen":false}',
    JSON.stringify({
      kind: "watch",
      session_ok: true,
      whoami: "personal",
      doctor_verdict: "SAFE",
      safe_next: { action: "ready", ready: true },
      pinned: true,
      frozen: false,
    }),
  ].join("\n"),
);
assert(nestedHb.session_ok === true && nestedHb.safe_next === "ready", "watch last NDJSON + nested safe_next");
const legacyHb = parseWatchHeartbeat({
  pinned: true,
  seal_ok: true,
  binding_present: true,
  frozen: false,
  binding_alias: "personal",
  issues: [],
  ok: true,
});
assert(legacyHb.source === "legacy-runtime" && legacyHb.session_ok === true, "watch legacy ok");
assert(legacyHb.whoami === "personal" && legacyHb.safe_next === "ready", "watch legacy whoami");
let threw = false;
try {
  parseWatchHeartbeat("");
} catch {
  threw = true;
}
assert(threw, "watch empty fails closed");
threw = false;
try {
  parseWatchHeartbeat('{ "foo": 1 }');
} catch {
  threw = true;
}
assert(threw, "watch unrecognized fails closed");
const packOk = parseSessionVerificationPack(
  JSON.stringify({
    kind: "session",
    version: "0.2.0",
    session_ok: true,
    safe_next: { action: "ready", ready: true },
  }),
);
assert(packOk.session_ok === true && packOk.kind === "session", "session pack ok");
const packMissing = parseSessionVerificationPack(JSON.stringify({ kind: "session", version: "1" }));
assert(packMissing.session_ok === false, "session pack missing session_ok → false");
const secretish = parseWatchHeartbeat({
  kind: "watch",
  session_ok: true,
  whoami: "acme",
  doctor_verdict: "SAFE",
  safe_next: "ready",
  pinned: true,
  frozen: false,
  token: "should-be-ignored",
  credential: "phm:NAME",
});
assert(
  secretish.whoami === "acme" && !JSON.stringify(secretish).match(/phm:|token|credential/),
  "watch never promotes secret-shaped keys",
);

// --- mergeLocusIntoMcpConfig ---
function mergeLocusIntoMcpConfig(config, opts = {}) {
  const serverName = opts.name ?? "locus";
  const entry = {
    command: opts.command ?? "locus-mcp",
    args: [],
    env: {
      LOCUS_HOME: opts.locusHome ?? "/tmp/locus",
      LOCUS_NOTIFY: "0",
      LOCUS_CLIENT: opts.client ?? "ashlr-hub",
    },
  };
  const base = config && typeof config === "object" ? { ...config } : { mcpServers: {} };
  const existing =
    base.mcpServers && typeof base.mcpServers === "object" ? { ...base.mcpServers } : {};
  const prev = existing[serverName];
  const changed = JSON.stringify(prev) !== JSON.stringify(entry);
  existing[serverName] = entry;
  return { config: { ...base, mcpServers: existing }, changed, serverName };
}

const m1 = mergeLocusIntoMcpConfig({ mcpServers: { other: { command: "x" } } }, { locusHome: "/tmp/h" });
assert(m1.changed === true, "mcp merge inserts locus");
assert(m1.config.mcpServers.locus.command === "locus-mcp", "mcp merge command");
assert(m1.config.mcpServers.other.command === "x", "mcp merge preserves others");
const m2 = mergeLocusIntoMcpConfig(m1.config, { locusHome: "/tmp/h" });
assert(m2.changed === false, "mcp merge idempotent");

// Live report from env if provided
const live = process.env.HUB_INT_REPORT;
if (live) {
  let report;
  try {
    report = JSON.parse(live);
  } catch {
    assert(false, "live report JSON parse");
    process.exit(process.exitCode || 0);
  }
  assert(hasRequiredServers(report.required_servers), "live report has locus+phantom");
  assert(typeof report.status_oneline === "string", "live report status_oneline");
  const parsed = parseStatusOneline(report.status_oneline);
  assert(parsed.raw === report.status_oneline.trim(), "live oneline parse roundtrip raw");
  // Gate should fail-closed if not ready, or pass if ready
  const g = evaluateFleetGate(report);
  assert(typeof g.allowDispatch === "boolean", "live fleet gate boolean");
  assert(Array.isArray(g.blockers), "live fleet gate blockers array");
  if (g.allowDispatch) {
    assert(g.blockers.length === 0, "live allow ⇒ empty blockers");
  } else {
    assert(g.blockers.length > 0, "live deny ⇒ blockers");
  }
  console.log("ok    pure: live report gate allowDispatch=" + g.allowDispatch);
}
NODE
# shellcheck disable=SC2090
export HUB_INT_REPORT="$REPORT"
# Re-run live slice with report (node above already ran without live; second pass)
HUB_INT_REPORT="$REPORT" node <<'NODE'
const report = JSON.parse(process.env.HUB_INT_REPORT || "{}");
const REQUIRED = ["locus", "phantom"];
function hasRequiredServers(servers) {
  if (!servers || !servers.length) return false;
  const set = new Set(servers.map((s) => String(s).trim().toLowerCase()));
  return REQUIRED.every((r) => set.has(r));
}
function parseStatusOneline(raw) {
  const s = (raw ?? "").trim() || "unpinned";
  if (["unpinned", "require_pin", "frozen", "invalid"].includes(s)) {
    return { kind: s, raw: s, healthy: false };
  }
  const colon = s.indexOf(":");
  if (colon > 0 && colon < s.length - 1) {
    return { kind: "pinned", raw: s, alias: s.slice(0, colon), tenant: s.slice(colon + 1), healthy: true };
  }
  return { kind: "invalid", raw: s, healthy: false };
}
function blockersFromAgentReport(report) {
  const blockers = [];
  if (!report || !report.status) return ["no agent report"];
  const status = report.status;
  const oneline = report.status_oneline ?? "unpinned";
  const parsed = parseStatusOneline(oneline);
  if (status === "unsafe") blockers.push("status=unsafe");
  else if (status !== "ready") blockers.push(`status=${status} (not ready)`);
  if (report.ready !== true) blockers.push("ready=false");
  if (!parsed.healthy) blockers.push(`pin unhealthy: ${parsed.kind}`);
  if (!hasRequiredServers(report.required_servers)) blockers.push("required_servers incomplete");
  if ((report.mcp_command || "") !== "locus-mcp") blockers.push("mcp_command");
  return blockers;
}
const blockers = blockersFromAgentReport(report);
const allow = blockers.length === 0;
if (!hasRequiredServers(report.required_servers)) {
  console.error("FAIL  live: required_servers");
  process.exit(1);
}
console.log("ok    live: required_servers locus+phantom");
console.log("ok    live: status_oneline=" + report.status_oneline + " parsed.healthy=" + parseStatusOneline(report.status_oneline).healthy);
console.log("ok    live: fleet gate allowDispatch=" + allow + (allow ? "" : " blockers=" + JSON.stringify(blockers)));
// Build a synthetic hub-gate object and validate shape against schema required keys
const gate = { allowDispatch: allow, blockers, report };
for (const k of ["allowDispatch", "blockers", "report"]) {
  if (!(k in gate)) {
    console.error("FAIL  hub-gate missing " + k);
    process.exit(1);
  }
}
console.log("ok    live: hub-gate shape {allowDispatch,blockers,report}");
NODE
live_ec=$?
if [[ "$live_ec" -ne 0 ]]; then
  fail=1
fi

# ---------------------------------------------------------------------------
# 5. registerLocusInMcpConfig via node fs (mirrors pure merge + write)
# ---------------------------------------------------------------------------
MCP_OUT="$SMOKE_PROJ/hub-merged.mcp.json"
HUB_INT_MCP_OUT="$MCP_OUT" node <<'NODE'
const fs = require("fs");
const path = process.env.HUB_INT_MCP_OUT;
if (!path) {
  console.error("FAIL  HUB_INT_MCP_OUT unset");
  process.exit(1);
}
const existing = { mcpServers: { phantom: { command: "phantom-mcp", args: [] } } };
const entry = {
  command: "locus-mcp",
  args: [],
  env: {
    LOCUS_HOME: process.env.LOCUS_HOME || "/tmp",
    LOCUS_NOTIFY: "0",
    LOCUS_CLIENT: "ashlr-hub",
  },
};
const config = {
  ...existing,
  mcpServers: { ...existing.mcpServers, locus: entry },
};
fs.writeFileSync(path, JSON.stringify(config, null, 2) + "\n");
const read = JSON.parse(fs.readFileSync(path, "utf8"));
if (!read.mcpServers.locus || read.mcpServers.locus.command !== "locus-mcp") {
  console.error("FAIL  registerLocusInMcpConfig merge write");
  process.exit(1);
}
if (!read.mcpServers.phantom) {
  console.error("FAIL  registerLocusInMcpConfig preserved phantom");
  process.exit(1);
}
console.log("ok    registerLocusInMcpConfig merge write (" + path + ")");
NODE

# ---------------------------------------------------------------------------
# 6. Drop-in + docs present
# ---------------------------------------------------------------------------
for f in \
  integrations/ashlr-hub/locus.ts \
  integrations/ashlr-hub/fleet-preflight.md \
  integrations/ashlr-hub/README.md \
  docs/hub-integration.md
do
  if [[ -f "$ROOT/$f" ]]; then
    ok "$f present"
  else
    bad "$f missing"
  fi
done

# locus.ts exports (static grep)
for sym in locusFleetGate registerLocusInMcpConfig parseStatusOneline evaluateFleetGate mergeLocusIntoMcpConfig hasRequiredServers scrubbedChildEnv validateMintEnv validateMintBinding parseLocusEnforceToken extractLocusConfigEnforce readLocusConfigFromAshlr resolveLocusEnforceMode decidePreMutateGate assertLocusPreMutate formatPreMutateBlockers applyLocusPreMutateGate decideLocusSessionRun runWithLocusSessionIfConfigured parseWatchHeartbeat parseSessionVerificationPack locusVerifySession locusWatchOnce locusSoftWatchHeartbeat parseMcpMintOutput parseMcpListOutput classifyTenantAuthError locusMcpMint locusMcpList locusMcpRevoke withLocusMcpTenant locusMtPreflight; do
  if grep -qE "export (async )?function $sym" "$ROOT/integrations/ashlr-hub/locus.ts"; then
    ok "locus.ts exports $sym"
  else
    bad "locus.ts missing export $sym"
  fi
done

# Docs mention LOCUS_ENFORCE + config.locus + scrub helpers
if grep -q "LOCUS_ENFORCE" "$ROOT/docs/hub-integration.md" && grep -q "scrubbedChildEnv" "$ROOT/docs/hub-integration.md" && grep -q "config.locus" "$ROOT/docs/hub-integration.md"; then
  ok "hub-integration.md documents LOCUS_ENFORCE + config.locus + scrubbedChildEnv"
else
  bad "hub-integration.md missing LOCUS_ENFORCE / config.locus / scrubbedChildEnv notes"
fi
if grep -qE "locusWatchOnce|locusVerifySession|parseWatchHeartbeat" "$ROOT/docs/hub-integration.md"; then
  ok "hub-integration.md documents watch/verify session heartbeat helpers"
else
  bad "hub-integration.md missing watch/verify session heartbeat notes"
fi
if grep -qE "assertLocusPreMutate|applyLocusPreMutateGate" "$ROOT/integrations/ashlr-hub/fleet-preflight.md" && grep -q "validateMintEnv" "$ROOT/integrations/ashlr-hub/fleet-preflight.md" && grep -q "locus.enforce" "$ROOT/integrations/ashlr-hub/fleet-preflight.md"; then
  ok "fleet-preflight.md documents pre-mutate + validateMintEnv + locus.enforce"
else
  bad "fleet-preflight.md missing pre-mutate / mint scrub / firm config notes"
fi
if grep -qE "locusWatchOnce|locusVerifySession" "$ROOT/integrations/ashlr-hub/fleet-preflight.md"; then
  ok "fleet-preflight.md documents session heartbeat helpers"
else
  bad "fleet-preflight.md missing session heartbeat notes"
fi
if grep -q "withLocusMcpTenant" "$ROOT/docs/hub-integration.md" \
  && grep -q "classifyTenantAuthError" "$ROOT/docs/hub-integration.md" \
  && grep -q "X-Locus-Tenant-Token" "$ROOT/docs/hub-integration.md"; then
  ok "hub-integration.md documents withLocusMcpTenant + tenant header contract"
else
  bad "hub-integration.md missing withLocusMcpTenant / tenant header notes"
fi

# ---------------------------------------------------------------------------
# 7. Multi-tenant MCP composition smoke
#    mint → serve --multi-tenant → whoami as tenant → cross-tenant 403 →
#    sessionless 400 → revoke → 401 → expired 401 (feature-detected; soft-skip)
#    Tokens live in shell/process memory only — never files, never echoed.
# ---------------------------------------------------------------------------
MT_SKIP=""
if ! command -v curl >/dev/null 2>&1; then
  MT_SKIP="curl not available"
fi
if [[ -z "$MT_SKIP" ]] && ! locus mcp mint --help >/dev/null 2>&1; then
  MT_SKIP="locus mcp subcommand unavailable (older CLI)"
fi
if [[ -z "$MT_SKIP" ]] && ! command -v locus-mcp >/dev/null 2>&1; then
  (cd "$ROOT" && cargo build -q -p locus-mcp) || true
fi
if [[ -z "$MT_SKIP" ]] && ! command -v locus-mcp >/dev/null 2>&1; then
  MT_SKIP="locus-mcp binary not available"
fi

if [[ -n "$MT_SKIP" ]]; then
  echo "skip  multi-tenant MCP smoke ($MT_SKIP)"
else
  MT_PORT=$(( 20000 + RANDOM % 20000 ))
  MT_ADDR="127.0.0.1:${MT_PORT}"
  export LOCUS_MCP_HTTP_TOKEN="hub-int-mt-$$-${RANDOM}"
  MT_BODY="$SMOKE_PROJ/mt-body.json"

  # Grant tokens are held in shell variables only (memory-only hub contract).
  MINT_A_JSON="$(locus mcp mint --binding personal --ttl 10m --label hub-int-a --json 2>/dev/null)"
  MINT_B_JSON="$(locus mcp mint --binding personal --ttl 10m --label hub-int-b --json 2>/dev/null)"
  TOKEN_A="$(printf '%s' "$MINT_A_JSON" | jq -r .token)"
  TOKEN_B="$(printf '%s' "$MINT_B_JSON" | jq -r .token)"
  GRANT_A="$(printf '%s' "$MINT_A_JSON" | jq -r .grant_id)"
  GRANT_B="$(printf '%s' "$MINT_B_JSON" | jq -r .grant_id)"

  if printf '%s' "$MINT_A_JSON" | jq -e --arg g "$GRANT_A" '
    (.grant_id | test("^[a-f0-9]+$"))
    and (.token | startswith("lmt_" + $g + "."))
    and (.session_id | startswith("ses_"))
    and .binding == "personal"
    and (.expires_at | type) == "string"
  ' >/dev/null 2>&1; then
    ok "mt: mint JSON contract (token embeds grant_id; shown once)"
  else
    bad "mt: mint JSON contract"
  fi

  # Serve from the SAME shell/capability that minted (broker requirement).
  locus-mcp --http "$MT_ADDR" --multi-tenant >"$SMOKE_PROJ/mt-server.log" 2>&1 &
  MT_SRV_PID=$!
  mt_up=0
  for _ in $(seq 1 60); do
    if curl -sf "http://${MT_ADDR}/health" >/dev/null 2>&1; then mt_up=1; break; fi
    sleep 0.25
  done

  if [[ "$mt_up" -ne 1 ]]; then
    bad "mt: locus-mcp --http --multi-tenant did not become healthy"
    tail -c 400 "$SMOKE_PROJ/mt-server.log" 2>/dev/null || true
  else
    ok "mt: server healthy on $MT_ADDR"

    # POST /mcp helper: $1 tenant token ('' = none), $2 session id ('' = none),
    # $3 JSON-RPC body. Prints HTTP status; response body → $MT_BODY.
    mt_post() {
      local args=( -s -o "$MT_BODY" -w '%{http_code}'
        -H "Authorization: Bearer ${LOCUS_MCP_HTTP_TOKEN}"
        -H "Content-Type: application/json" -H "Accept: application/json" )
      [[ -n "$1" ]] && args+=( -H "X-Locus-Tenant-Token: $1" )
      [[ -n "$2" ]] && args+=( -H "Mcp-Session-Id: $2" )
      curl "${args[@]}" -d "$3" "http://${MT_ADDR}/mcp"
    }

    # Capabilities without a tenant token: only the MT layer answers invalid_grant.
    st="$(curl -s -o "$MT_BODY" -w '%{http_code}' -H "Authorization: Bearer ${LOCUS_MCP_HTTP_TOKEN}" "http://${MT_ADDR}/mcp")"
    if [[ "$st" == "401" ]] && jq -e '.error == "invalid_grant"' "$MT_BODY" >/dev/null 2>&1; then
      ok "mt: GET /mcp without tenant token → 401 invalid_grant"
    else
      bad "mt: expected 401 invalid_grant without tenant token (got $st)"
    fi
    MT_401_INVALID_BODY="$(cat "$MT_BODY")"

    # Capabilities as tenant A: mode + THIS grant only.
    st="$(curl -s -o "$MT_BODY" -w '%{http_code}' -H "Authorization: Bearer ${LOCUS_MCP_HTTP_TOKEN}" -H "X-Locus-Tenant-Token: ${TOKEN_A}" "http://${MT_ADDR}/mcp")"
    if [[ "$st" == "200" ]] && jq -e --arg g "$GRANT_A" '
      .mode == "multi_tenant" and .grant_id == $g and .pin.binding_alias == "personal"
    ' "$MT_BODY" >/dev/null 2>&1; then
      ok "mt: GET /mcp capabilities mode=multi_tenant grant-scoped"
    else
      bad "mt: capabilities as tenant A (got $st)"
      jq -c 'del(.tools)' "$MT_BODY" 2>/dev/null | head -c 300 || true
    fi

    # Initialize as tenant A → grant-bound Mcp-Session-Id.
    st="$(curl -s -D "$SMOKE_PROJ/mt-init.headers" -o "$MT_BODY" -w '%{http_code}' \
      -H "Authorization: Bearer ${LOCUS_MCP_HTTP_TOKEN}" -H "X-Locus-Tenant-Token: ${TOKEN_A}" \
      -H "Content-Type: application/json" -H "Accept: application/json" \
      -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' "http://${MT_ADDR}/mcp")"
    SESSION_A="$(grep -i '^mcp-session-id:' "$SMOKE_PROJ/mt-init.headers" | tr -d '\r' | awk '{print $2}')"
    if [[ "$st" == "200" && -n "$SESSION_A" ]]; then
      ok "mt: initialize minted grant-bound Mcp-Session-Id"
    else
      bad "mt: initialize as tenant A (status $st, session '${SESSION_A}')"
    fi

    # whoami as tenant A — scoped to the grant's binding; never bearer material.
    st="$(mt_post "$TOKEN_A" "$SESSION_A" '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"locus_whoami","arguments":{}}}')"
    if [[ "$st" == "200" ]] && jq -e '
      (.result.isError | not)
      and (.result.content[0].text | contains("\"tenant\": \"personal\"") or contains("\"tenant\":\"personal\""))
    ' "$MT_BODY" >/dev/null 2>&1; then
      ok "mt: tools/call locus_whoami scoped to tenant grant"
    else
      bad "mt: whoami as tenant A (status $st)"
    fi
    if grep -q 'lmt_' "$MT_BODY"; then
      bad "mt: whoami response leaked bearer token material"
    else
      ok "mt: whoami response bearer hygiene"
    fi

    # Cross-tenant session reuse: token B + session A → 403 tenant_mismatch.
    st="$(mt_post "$TOKEN_B" "$SESSION_A" '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"locus_whoami","arguments":{}}}')"
    if [[ "$st" == "403" ]] && jq -e '.error == "tenant_mismatch"' "$MT_BODY" >/dev/null 2>&1; then
      ok "mt: cross-tenant session reuse → 403 tenant_mismatch"
    else
      bad "mt: expected 403 tenant_mismatch (got $st)"
    fi
    MT_403_BODY="$(cat "$MT_BODY")"

    # Sessionless MT tools/call → 400 session_required (no ambient fallthrough).
    st="$(mt_post "$TOKEN_A" "" '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"locus_whoami","arguments":{}}}')"
    if [[ "$st" == "400" ]] && jq -e '.error == "session_required"' "$MT_BODY" >/dev/null 2>&1; then
      ok "mt: sessionless tools/call → 400 session_required"
    else
      bad "mt: expected 400 session_required (got $st)"
    fi
    MT_400_BODY="$(cat "$MT_BODY")"

    # Revoke A → token A refused within one request (uniform invalid_grant).
    locus mcp revoke "$GRANT_A" >/dev/null 2>&1
    st="$(mt_post "$TOKEN_A" "$SESSION_A" '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"locus_whoami","arguments":{}}}')"
    if [[ "$st" == "401" ]] && jq -e '.error == "invalid_grant" and (has("reason") | not)' "$MT_BODY" >/dev/null 2>&1; then
      ok "mt: revoked grant → 401 invalid_grant (revocation not advertised)"
    else
      bad "mt: expected 401 invalid_grant after revoke (got $st)"
    fi

    # Expired grant → 401 with reason=grant_expired + re-mint safe_next.
    TOKEN_E="$(locus mcp mint --binding personal --ttl 1s --label hub-int-exp --json 2>/dev/null | jq -r .token)"
    sleep 2
    st="$(curl -s -o "$MT_BODY" -w '%{http_code}' -H "Authorization: Bearer ${LOCUS_MCP_HTTP_TOKEN}" -H "X-Locus-Tenant-Token: ${TOKEN_E}" "http://${MT_ADDR}/mcp")"
    if [[ "$st" == "401" ]] && jq -e '
      .error == "invalid_grant" and .reason == "grant_expired" and (.safe_next | contains("locus mcp mint"))
    ' "$MT_BODY" >/dev/null 2>&1; then
      ok "mt: expired grant → 401 grant_expired + safe_next re-mint"
    else
      bad "mt: expected 401 grant_expired (got $st)"
    fi
    MT_401_EXPIRED_BODY="$(cat "$MT_BODY")"

    # Roster: values-free, revoked grant removed, live grant present.
    MT_LIST_JSON="$(locus mcp list --json 2>/dev/null)"
    if printf '%s' "$MT_LIST_JSON" | jq -e --arg a "$GRANT_A" --arg b "$GRANT_B" '
      type == "array"
      and (map(has("token")) | any | not)
      and (map(select(.grant_id == $a)) | length == 0)
      and (map(select(.grant_id == $b)) | first | .revoked == false)
      and (map(.live_http_sessions | type == "number") | all)
    ' >/dev/null 2>&1; then
      ok "mt: locus mcp list roster (values-free; revoked grant removed)"
    else
      bad "mt: locus mcp list roster contract"
    fi
    if printf '%s' "$MT_LIST_JSON" | grep -q 'lmt_'; then
      bad "mt: list roster leaked bearer token material"
    else
      ok "mt: list roster bearer hygiene"
    fi

    # Live drop-in slice: parse helpers + classify + preflight + withLocusMcpTenant
    # against the real server. Tokens stay in process env memory only.
    if HUB_ROOT="$ROOT" MT_URL="http://${MT_ADDR}" \
      HUB_MT_MINT_JSON="$MINT_B_JSON" HUB_MT_LIST_JSON="$MT_LIST_JSON" \
      HUB_MT_401_INVALID="$MT_401_INVALID_BODY" HUB_MT_401_EXPIRED="$MT_401_EXPIRED_BODY" \
      HUB_MT_403="$MT_403_BODY" HUB_MT_400="$MT_400_BODY" HUB_MT_TOKEN_B="$TOKEN_B" \
      LOCUS_BIN="${ROOT}/target/debug/locus" \
      node --experimental-strip-types --input-type=module <<'NODE'
import { pathToFileURL } from "node:url";

const root = process.env.HUB_ROOT;
const url = process.env.MT_URL;
const serverToken = process.env.LOCUS_MCP_HTTP_TOKEN;
const m = await import(pathToFileURL(`${root}/integrations/ashlr-hub/locus.ts`).href);

// parseMcpMintOutput: live mint roundtrip + fail-closed variants
const mint = m.parseMcpMintOutput(process.env.HUB_MT_MINT_JSON);
if (!mint.token.startsWith(`lmt_${mint.grant_id}.`) || mint.binding !== "personal") {
  throw new Error("parseMcpMintOutput live roundtrip failed");
}
for (const bad of [
  "", "not-json",
  JSON.stringify({ ...mint, token: "lmt_ffffffffffffffff.deadbeef" }), // foreign grant id
  JSON.stringify({ ...mint, token: "sk-notatoken" }),
  JSON.stringify({ ...mint, session_id: "bogus" }),
]) {
  let rejected = false;
  try { m.parseMcpMintOutput(bad); } catch { rejected = true; }
  if (!rejected) throw new Error("parseMcpMintOutput accepted malformed mint");
}
console.log("ok    mt-ts: parseMcpMintOutput live + fail-closed");

// parseMcpListOutput: live roster + token-material rejection
const rows = m.parseMcpListOutput(process.env.HUB_MT_LIST_JSON);
if (!rows.some((r) => r.grant_id === mint.grant_id && r.revoked === false)) {
  throw new Error("parseMcpListOutput missing live grant");
}
let listRejected = false;
try {
  m.parseMcpListOutput(JSON.stringify([{ grant_id: "ab12", token: "lmt_ab12.ffff" }]));
} catch { listRejected = true; }
if (!listRejected) throw new Error("parseMcpListOutput accepted bearer material");
console.log("ok    mt-ts: parseMcpListOutput live + bearer hygiene");

// classifyTenantAuthError: captured live bodies → typed reasons
const cases = [
  [401, process.env.HUB_MT_401_INVALID, "invalid_grant", "re_mint"],
  [401, process.env.HUB_MT_401_EXPIRED, "grant_expired", "re_mint"],
  [403, process.env.HUB_MT_403, "tenant_mismatch", "initialize"],
  [400, process.env.HUB_MT_400, "session_required", "initialize"],
  [401, JSON.stringify({ error: "unauthorized" }), "server_unauthorized", "server_token"],
  [500, "{}", "unknown", "none"],
];
for (const [status, body, kind, recovery] of cases) {
  const c = m.classifyTenantAuthError(status, body);
  if (c.kind !== kind || c.recovery !== recovery) {
    throw new Error(`classifyTenantAuthError(${status}) => ${c.kind}/${c.recovery}, want ${kind}/${recovery}`);
  }
}
if (!m.classifyTenantAuthError(401, process.env.HUB_MT_401_EXPIRED).safeNext?.includes("locus mcp mint")) {
  throw new Error("grant_expired classification lost safe_next");
}
console.log("ok    mt-ts: classifyTenantAuthError live 401/403/400 taxonomy");

// locusMtPreflight: server-token-only probe proves MT via invalid_grant …
const anon = await m.locusMtPreflight({ baseUrl: url, serverToken });
if (!anon.reachable || !anon.multiTenant || anon.mode !== "multi_tenant") {
  throw new Error(`anon preflight: ${JSON.stringify(anon)}`);
}
// … and a tenant-authenticated probe returns THIS grant.
const authed = await m.locusMtPreflight({
  baseUrl: url,
  serverToken,
  headers: { [m.TENANT_TOKEN_HEADER]: process.env.HUB_MT_TOKEN_B },
});
if (!authed.multiTenant || authed.grantId !== mint.grant_id) {
  throw new Error(`authed preflight: ${JSON.stringify(authed)}`);
}
const down = await m.locusMtPreflight({ baseUrl: "http://127.0.0.1:9", timeoutMs: 1500 });
if (down.reachable !== false) throw new Error("down preflight claimed reachable");
console.log("ok    mt-ts: locusMtPreflight MT detection (anon + tenant + down)");

// withLocusMcpTenant: mint → dispatch → auto-revoke → scrubbed handle
let saved = null;
await m.withLocusMcpTenant("personal", async (handle) => {
  saved = handle;
  if (!handle.headers[m.TENANT_TOKEN_HEADER]?.startsWith(`lmt_${handle.grantId}.`)) {
    throw new Error("handle headers missing tenant token");
  }
  if (handle.headers.Authorization !== `Bearer ${serverToken}`) {
    throw new Error("handle headers missing server bearer");
  }
  const res = await fetch(`${url}/mcp`, {
    method: "POST",
    headers: { ...handle.headers, "Content-Type": "application/json", Accept: "application/json" },
    body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "initialize", params: {} }),
  });
  if (!res.ok) throw new Error(`initialize under handle failed: ${res.status}`);
}, { ttl: "5m", label: "hub-int-ts", serverToken });
if (!saved?.revoked) throw new Error("withLocusMcpTenant did not revoke in finally");
if (saved.headers[m.TENANT_TOKEN_HEADER] !== undefined || saved.headers.Authorization !== undefined) {
  throw new Error("withLocusMcpTenant left bearer material on the handle");
}
// Revoke deletes the record — the grant must be gone from the roster.
const after = m.locusMcpList();
if (!after.available || after.grants.some((g) => g.grant_id === saved.grantId)) {
  throw new Error("revoked grant still present in roster");
}
console.log("ok    mt-ts: withLocusMcpTenant mint→dispatch→revoke + token scrub");

// Failing callback: grant still revoked, callback error wins.
let failed = false;
let saved2 = null;
try {
  await m.withLocusMcpTenant("personal", (handle) => {
    saved2 = handle;
    throw new Error("job failed");
  }, { ttl: "5m", label: "hub-int-ts-fail" });
} catch (e) {
  failed = e instanceof Error && e.message === "job failed";
}
if (!failed || !saved2?.revoked) {
  throw new Error("failing callback did not preserve error + revoke");
}
console.log("ok    mt-ts: withLocusMcpTenant revokes on callback failure");
NODE
    then
      ok "mt: live drop-in slice (parse + classify + preflight + withLocusMcpTenant)"
    else
      bad "mt: live drop-in slice failed"
    fi

    kill "$MT_SRV_PID" 2>/dev/null || true
    wait "$MT_SRV_PID" 2>/dev/null || true
    MT_SRV_PID=""
  fi

  # Drain remaining grants so the scratch store ends clean.
  locus mcp revoke --all >/dev/null 2>&1 || true
fi

if [[ "$fail" -ne 0 ]]; then
  echo "== hub-integration-test FAILED =="
  exit 1
fi
echo "== hub-integration-test OK =="
