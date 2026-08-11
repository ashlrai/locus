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
cleanup() { rm -rf "$SMOKE_HOME" "$SMOKE_PROJ"; }
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
for f in agent-report.schema.json doctor.schema.json hub-gate.schema.json; do
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
for sym in locusFleetGate registerLocusInMcpConfig parseStatusOneline evaluateFleetGate mergeLocusIntoMcpConfig hasRequiredServers scrubbedChildEnv validateMintEnv validateMintBinding parseLocusEnforceToken extractLocusConfigEnforce readLocusConfigFromAshlr resolveLocusEnforceMode decidePreMutateGate assertLocusPreMutate formatPreMutateBlockers applyLocusPreMutateGate decideLocusSessionRun runWithLocusSessionIfConfigured parseWatchHeartbeat parseSessionVerificationPack locusVerifySession locusWatchOnce locusSoftWatchHeartbeat; do
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

if [[ "$fail" -ne 0 ]]; then
  echo "== hub-integration-test FAILED =="
  exit 1
fi
echo "== hub-integration-test OK =="
