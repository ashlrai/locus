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
