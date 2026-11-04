// Run: node --test scripts/e2e-security-workspace-selection.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { lstatSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'e2e.sh');

test('SECURITY_WS is created directly beneath the configured TMPDIR and cleaned up', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-security-ws-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'custom tmp [*]');
  const mainHome = join(root, 'main home');
  const observed = join(root, 'workspace-path');
  const template = join(root, 'template');
  mkdirSync(scratch);
  mkdirSync(mainHome);
  writeFileSync(join(scratch, 'keep'), 'unrelated');

  const source = readFileSync(script, 'utf8');
  const cleanup = source.match(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m)?.[0];
  const create = source.match(/^create_security_workspace\(\) \{[\s\S]*?^\}/m)?.[0];
  assert.ok(cleanup && create, 'e2e.sh must define the security workspace lifecycle');
  const result = spawnSync('bash', ['-c', `set -euo pipefail
${cleanup}
${create}
mktemp() {
  [[ "$#" == 2 && "$1" == -d ]] || exit 70
  printf '%s' "$2" > "$TEST_TEMPLATE"
  command mktemp "$@"
}
LOCUS_HOME="$TEST_HOME"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
trap 'e2e_cleanup' EXIT
create_security_workspace
[[ -d "$SECURITY_WS" && "\${SECURITY_WS%/*}" == "$TMPDIR" ]] || exit 71
printf 'fixture' > "$SECURITY_WS/probe"
[[ "$(cat "$SECURITY_WS/probe")" == fixture ]] || exit 72
printf '%s' "$SECURITY_WS" > "$TEST_OBSERVED"
`], {
    env: { ...process.env, TMPDIR: scratch, TEST_HOME: mainHome, TEST_TEMPLATE: template, TEST_OBSERVED: observed },
    encoding: 'utf8',
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(readFileSync(template, 'utf8'), `${scratch}/locus-security-ws.XXXXXX`);
  const workspace = readFileSync(observed, 'utf8');
  assert.equal(dirname(workspace), scratch);
  assert.match(workspace, /\/locus-security-ws\.[A-Za-z0-9]{6}$/);
  assert.throws(() => lstatSync(workspace), { code: 'ENOENT' });
  assert.deepEqual(readdirSync(scratch), ['keep']);
  assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'unrelated');
});

// The script's own section 2e rehearsal: SECURITY_WS beneath a supplied
// temporary root whose name contains spaces (including a doubled space). The
// probes run the block verbatim; stubs provide only log/ok/die and, where
// noted, a mktemp that misplaces the directory.
function section2e() {
  const source = readFileSync(script, 'utf8');
  const start = source.indexOf('log "2e.');
  const endMarker = 'ok "SECURITY_WS creation succeeds beneath a TMPDIR containing spaces"';
  const end = source.indexOf(endMarker, start);
  assert.ok(start >= 0 && end > start, 'e2e.sh must contain the section 2e rehearsal');
  return source.slice(start, end + endMarker.length);
}

function rehearse2e(t, shim = '') {
  const root = mkdtempSync(join(tmpdir(), 'locus-security-ws-2e-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const source = readFileSync(script, 'utf8');
  const cleanup = source.match(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m)?.[0];
  const create = source.match(/^create_security_workspace\(\) \{[\s\S]*?^\}/m)?.[0];
  assert.ok(cleanup && create, 'e2e.sh must define the security workspace lifecycle');
  const home = join(root, 'main home');
  mkdirSync(home);
  const result = spawnSync('bash', ['-c', `set -euo pipefail
${cleanup}
${create}
${shim}
log() { :; }
ok() { printf 'ok %s\\n' "$*"; }
die() { printf 'FAIL %s\\n' "$*" >&2; exit 1; }
LOCUS_HOME="$1"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
${section2e()}
`, 'security-ws-2e', home], { env: { ...process.env, TMPDIR: root }, encoding: 'utf8' });
  return { result, home, root };
}

test('section 2e creates SECURITY_WS under a spaced TMPDIR and cleans it up', (t) => {
  const { result, home, root } = rehearse2e(t);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /ok SECURITY_WS creation succeeds beneath a TMPDIR containing spaces/);
  const nested = join(home, 'security  ws temp root');
  const entries = readdirSync(nested).sort();
  // The rehearsed workspace and the subshell home are gone; only the path record remains.
  assert.deepEqual(entries, ['security-workspace-path']);
  const observed = readFileSync(join(nested, 'security-workspace-path'), 'utf8');
  assert.equal(dirname(observed), nested);
  assert.match(observed, /\/locus-security-ws\.[A-Za-z0-9]{6}$/);
  assert.throws(() => lstatSync(observed), { code: 'ENOENT' });
  assert.ok(lstatSync(root).isDirectory());
});

test('section 2e fails when SECURITY_WS is not created under the requested parent', (t) => {
  const { result } = rehearse2e(t, `mktemp() { command mktemp -d "\${TEST_ELSEWHERE:-/tmp}/locus-security-ws.XXXXXX"; }
TEST_ELSEWHERE="$(command mktemp -d)"`);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /FAIL SECURITY_WS was not created beneath the space-containing TMPDIR/);
});
