// Run: node --test scripts/e2e-graph-isolation.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const source = readFileSync(join(root, 'scripts/e2e.sh'), 'utf8');
const graph = source.slice(source.indexOf('# ── 16. graph'), source.indexOf('# ── 17. ci'));
const cleanup = source.slice(source.indexOf('e2e_cleanup() {'), source.indexOf("trap 'e2e_cleanup' EXIT"));
assert.ok(graph.includes('GRAPH_HOME=') && cleanup.includes('"${GRAPH_HOME:-}"'));

for (const scenario of ['same-home', 'success', 'init-failure', 'import-failure', 'cleanup-failure']) {
  test(`graph ${scenario} uses and cleans the expected home`, (t) => {
    const fixture = mkdtempSync(join(tmpdir(), 'locus-graph-section-'));
    t.after(() => rmSync(fixture, { recursive: true, force: true }));
    const scratch = join(fixture, 'temp with spaces');
    const home = join(fixture, 'main-home');
    const record = join(fixture, 'record');
    mkdirSync(scratch);
    mkdirSync(home);
    writeFileSync(join(scratch, 'keep'), 'unrelated');
    const harness = `#!/bin/bash
set -euo pipefail
export LOCUS_HOME="$TEST_MAIN_HOME"
GRAPH_HOME="" WS_DIR="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
log() { :; }
ok() { :; }
skip() { exit 90; }
die() { printf '%s\\n' "$*" >&2; exit 1; }
has_cmd() { return 0; }
rm() {
  if [[ "$TEST_SCENARIO" == cleanup-failure && "$#" -eq 2 && "$2" == "$GRAPH_HOME" ]]; then
    return 0
  fi
  command rm "$@"
}
locus() {
  case "$1:\${2:-}" in
    graph:export) printf 'LOCUSGRAPH00' > "$4" ;;
    graph:import)
      if [[ "$LOCUS_HOME" == "$TEST_MAIN_HOME" ]]; then
        [[ "$TEST_SCENARIO" == same-home ]]
      else
        [[ -d "$LOCUS_HOME" && "$LOCUS_GRAPH_PASSPHRASE" == graph-test ]] || return 1
        [[ "$(cat "$TEST_RECORD")" == "$LOCUS_HOME" ]] || return 1
        touch "$LOCUS_HOME/import-sentinel"
        [[ "$TEST_SCENARIO" != import-failure ]]
      fi ;;
    init:*)
      [[ -d "$LOCUS_HOME" && "$LOCUS_GRAPH_PASSPHRASE" == graph-test ]] || return 1
      printf '%s' "$LOCUS_HOME" > "$TEST_RECORD"
      [[ "$TEST_SCENARIO" != init-failure ]] ;;
    *) return 1 ;;
  esac
}
${cleanup}
trap 'e2e_cleanup' EXIT
${graph}
if [[ "$TEST_SCENARIO" == success ]]; then
  [[ ! -e "$(cat "$TEST_RECORD")" ]] || exit 2
fi
`;
    const result = spawnSync('bash', ['-c', harness], {
      env: { ...process.env, TMPDIR: scratch, TEST_MAIN_HOME: home, TEST_RECORD: record,
        TEST_SCENARIO: scenario, LOCUS_GRAPH_PASSPHRASE: 'graph-test' },
      encoding: 'utf8',
    });
    assert.equal(result.status, scenario.endsWith('failure') ? 1 : 0);
    assert.equal(result.signal, null);
    if (scenario === 'cleanup-failure') {
      assert.match(result.stderr, /graph import temporary home was not removed/);
    }
    assert.deepEqual(readdirSync(scratch), ['keep']);
    if (scenario === 'same-home') {
      assert.throws(() => readFileSync(record), { code: 'ENOENT' });
    } else {
      const graphHome = readFileSync(record, 'utf8');
      assert.equal(dirname(graphHome), scratch);
      assert.match(graphHome, /\/locus-graph-imp\.[^/]+$/);
    }
  });
}
