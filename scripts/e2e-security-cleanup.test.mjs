// Run: node --test scripts/e2e-security-cleanup.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { chmodSync, existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const source = readFileSync(join(root, 'scripts', 'e2e.sh'), 'utf8');

// Real pieces of e2e.sh: the exit trap, the two creators, and the opening of
// section 24 (creation, containment check, first binding probes). Only the
// locus binary is a stub, so a probe failure goes through the script's own
// die() and the script's own EXIT trap.
function piece(re, what) {
  const m = source.match(re)?.[0];
  assert.ok(m, `e2e.sh must contain ${what}`);
  return m;
}
function section24Head() {
  const start = source.indexOf('log "24.');
  const endMarker = 'ok "binding add rejects conflicting content and preserves the existing file"';
  const end = source.indexOf(endMarker, start);
  assert.ok(start >= 0 && end > start, 'e2e.sh must contain the section 24 binding probes');
  return source.slice(start, end + endMarker.length);
}

// Stub locus: `binding add` is idempotent for identical input and rejects a
// different account, unless STUB_MODE=mutate, where every add rewrites the
// file so the "identical retry changed existing file" probe must fail.
const STUB = `#!/usr/bin/env bash
case "$1" in
  init) mkdir -p "$LOCUS_HOME/bindings" ;;
  binding)
    f="$LOCUS_HOME/bindings/conflict-probe.toml"
    acct=""; while [[ $# -gt 0 ]]; do [[ "$1" == --account ]] && acct="$2"; shift; done
    if [[ "\${STUB_MODE:-}" == mutate ]]; then date +%s%N > "$f"; exit 0; fi
    if [[ ! -e "$f" ]]; then echo "$acct" > "$f"
    elif [[ "$(cat "$f")" != "$acct" ]]; then echo "binding 'conflict-probe' already exists" >&2; exit 1
    fi ;;
esac
`;

function run(t, mode) {
  const dir = mkdtempSync(join(tmpdir(), 'locus-e2e-security-cleanup-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const scratch = join(dir, 'scratch');
  const mainHome = join(dir, 'main-home');
  const stub = join(dir, 'locus-stub');
  const seen = join(dir, 'seen');
  mkdirSync(scratch);
  mkdirSync(mainHome);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  writeFileSync(stub, STUB);
  chmodSync(stub, 0o755);
  const script = `set -euo pipefail
${piece(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m, 'e2e_cleanup')}
${piece(/^create_security_home\(\) \{[\s\S]*?^\}/m, 'create_security_home')}
${piece(/^create_security_workspace\(\) \{[\s\S]*?^\}/m, 'create_security_workspace')}
log() { :; }
ok() { printf 'ok %s\\n' "$*"; }
die() { printf 'FAIL %s\\n' "$*" >&2; exit 1; }
LOCUS_BIN="$STUB"
LOCUS_HOME="$MAIN_HOME"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
trap 'e2e_cleanup' EXIT
${section24Head()}
printf '%s\\n%s\\n' "$SECURITY_HOME" "$SECURITY_WS" > "$SEEN"
`;
  const result = spawnSync('bash', ['-c', script], {
    env: { ...process.env, TMPDIR: scratch, MAIN_HOME: mainHome, STUB: stub, SEEN: seen, STUB_MODE: mode },
    encoding: 'utf8',
    timeout: 30_000,
  });
  return { scratch, mainHome, seen, result };
}

test('security home and workspace are removed when section 24 passes', (t) => {
  const { scratch, mainHome, seen, result } = run(t, 'ok');
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /ok binding add rejects conflicting content/);
  const [home, ws] = readFileSync(seen, 'utf8').trim().split('\n');
  assert.match(home, /\/locus-security-e2e\./);
  assert.match(ws, /\/locus-security-ws\./);
  assert.ok(!existsSync(home), 'security home must be removed on exit');
  assert.ok(!existsSync(ws), 'security workspace must be removed on exit');
  assert.deepEqual(readdirSync(scratch), ['keep']);
  assert.ok(!existsSync(mainHome), 'the exit trap also removes the main home');
});

test('security home and workspace are removed when a security probe fails', (t) => {
  const { scratch, mainHome, result } = run(t, 'mutate');
  assert.equal(result.status, 1, 'the failed probe must fail the suite');
  assert.match(result.stderr, /FAIL identical binding retry changed existing file/);
  // `seen` is never written on failure, so identify leftovers by prefix.
  assert.deepEqual(readdirSync(scratch), ['keep'],
    'no locus-security-e2e.* or locus-security-ws.* directory may survive a failed probe');
  assert.ok(!existsSync(mainHome), 'the exit trap also removes the main home');
});

test('security home creation failure stops section 24 before security checks', (t) => {
  const dir = mkdtempSync(join(tmpdir(), 'locus-e2e-security-failure-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const mainHome = join(dir, 'main-home');
  const attempts = join(dir, 'attempts');
  const probe = join(dir, 'probe');
  const stub = join(dir, 'locus-stub');
  mkdirSync(mainHome);
  writeFileSync(stub, '#!/usr/bin/env bash\ntouch "$PROBE"\n');
  chmodSync(stub, 0o755);
  const script = `set -euo pipefail
${piece(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m, 'e2e_cleanup')}
mktemp() {
  printf '%s\\n' "\${2:-}" >> "$ATTEMPTS"
  if [[ "\${2:-}" == */locus-security-e2e.XXXXXX ]]; then return 73; fi
  command mktemp "$@"
}
${piece(/^create_security_home\(\) \{[\s\S]*?^\}/m, 'create_security_home')}
${piece(/^create_security_workspace\(\) \{[\s\S]*?^\}/m, 'create_security_workspace')}
log() { :; }
ok() { :; }
die() { exit 1; }
LOCUS_BIN="$STUB"
LOCUS_HOME="$MAIN_HOME"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
trap 'e2e_cleanup' EXIT
${section24Head()}
`;
  const result = spawnSync('bash', ['-c', script], {
    env: { ...process.env, TMPDIR: dir, MAIN_HOME: mainHome, STUB: stub, ATTEMPTS: attempts, PROBE: probe },
    encoding: 'utf8', timeout: 30_000,
  });
  assert.equal(result.signal, null);
  assert.equal(result.status, 73, 'the allocation failure must fail the suite');
  assert.deepEqual(readFileSync(attempts, 'utf8').trim().split('\n'),
    [join(dir, 'locus-security-e2e.XXXXXX')], 'workspace allocation must not run');
  assert.ok(!existsSync(probe), 'the security checks must not run');
  assert.ok(!existsSync(mainHome), 'the exit trap must still clean up');
});

test('security workspace creation failure removes the allocated security home', (t) => {
  const dir = mkdtempSync(join(tmpdir(), 'locus-e2e-security-ws-failure-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const scratch = join(dir, 'scratch with spaces');
  const mainHome = join(dir, 'main-home');
  const seen = join(dir, 'seen');
  const attempts = join(dir, 'attempts');
  const probe = join(dir, 'probe');
  const stub = join(dir, 'locus-stub');
  mkdirSync(scratch);
  mkdirSync(mainHome);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  writeFileSync(stub, '#!/usr/bin/env bash\ntouch "$PROBE"\n');
  chmodSync(stub, 0o755);
  const script = `set -euo pipefail
${piece(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m, 'e2e_cleanup')}
mktemp() {
  printf '%s\\n' "\${2:-}" >> "$ATTEMPTS"
  if [[ "\${2:-}" == */locus-security-ws.XXXXXX ]]; then
    [[ -d "$SECURITY_HOME" ]] || exit 74
    touch "$SECURITY_HOME/cleanup-sentinel"
    printf '%s' "$SECURITY_HOME" > "$SEEN"
    return 73
  fi
  command mktemp "$@"
}
${piece(/^create_security_home\(\) \{[\s\S]*?^\}/m, 'create_security_home')}
${piece(/^create_security_workspace\(\) \{[\s\S]*?^\}/m, 'create_security_workspace')}
log() { :; }
ok() { :; }
die() { exit 1; }
LOCUS_BIN="$STUB"
LOCUS_HOME="$MAIN_HOME"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
${piece(/^  trap 'e2e_cleanup' EXIT$/m, 'the EXIT cleanup trap')}
${section24Head()}
`;
  const result = spawnSync('bash', ['-c', script], {
    env: { ...process.env, TMPDIR: scratch, MAIN_HOME: mainHome, STUB: stub,
      ATTEMPTS: attempts, SEEN: seen, PROBE: probe },
    encoding: 'utf8', timeout: 30_000,
  });
  assert.ifError(result.error);
  assert.equal(result.signal, null);
  assert.equal(result.status, 73, 'workspace allocation failure must fail the suite');
  assert.deepEqual(readFileSync(attempts, 'utf8').trim().split('\n'), [
    join(scratch, 'locus-security-e2e.XXXXXX'),
    join(scratch, 'locus-security-ws.XXXXXX'),
  ]);
  const home = readFileSync(seen, 'utf8');
  assert.equal(dirname(home), scratch);
  assert.match(home, /\/locus-security-e2e\.[^/]+$/);
  assert.ok(!existsSync(home), 'the allocated SECURITY_HOME must be removed on failure');
  assert.deepEqual(readdirSync(scratch), ['keep'], 'cleanup must preserve unrelated files');
  assert.ok(!existsSync(probe), 'security checks must not run after allocation failure');
  assert.ok(!existsSync(mainHome), 'the exit trap must also remove the main home');
});
