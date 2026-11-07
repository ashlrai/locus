// Run: node --test scripts/e2e-graph-isolation.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, lstatSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const source = readFileSync(join(root, 'scripts/e2e.sh'), 'utf8');
const graph = source.slice(source.indexOf('# ── 16. graph'), source.indexOf('# ── 17. ci'));
const cleanup = source.slice(source.indexOf('e2e_cleanup() {'), source.indexOf("trap 'e2e_cleanup' EXIT"));
const createGraphHome = source.match(/^create_graph_home\(\) \{[\s\S]*?^\}/m)?.[0];
assert.ok(graph.includes('create_graph_home') && cleanup.includes('"${GRAPH_HOME:-}"'));
assert.ok(createGraphHome, 'missing create_graph_home function');

for (const scenario of ['same-home', 'success', 'init-failure', 'import-failure', 'cleanup-failure', 'relocated']) {
  test(`graph ${scenario} uses and cleans the expected home`, (t) => {
    const fixture = mkdtempSync(join(tmpdir(), 'locus-graph-section-'));
    t.after(() => rmSync(fixture, { recursive: true, force: true }));
    const scratch = join(fixture, 'temp with spaces');
    const home = join(fixture, 'main-home');
    const record = join(fixture, 'record');
    const importRecord = join(fixture, 'import-record');
    const rmObserved = join(fixture, 'rm-observed');
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
if [[ "$TEST_SCENARIO" == relocated ]]; then
  mktemp() { command mktemp -d "$TEST_MAIN_HOME/locus-graph-imp.XXXXXX"; }
fi
rm() {
  if [[ "$TEST_SCENARIO" == cleanup-failure && "$#" -eq 2 && "$2" == "$GRAPH_HOME" ]]; then
    return 0
  fi
  if [[ "$#" -eq 2 && "$1" == -rf && "$2" == "$GRAPH_HOME" && -f "$2/import-sentinel" ]]; then
    printf '%s' "$2" > "$TEST_RM_OBSERVED"
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
        printf '%s' "$LOCUS_HOME" > "$TEST_IMPORT_RECORD"
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
${createGraphHome}
trap 'e2e_cleanup' EXIT
${graph}
if [[ "$TEST_SCENARIO" == success ]]; then
  [[ ! -e "$(cat "$TEST_RECORD")" ]] || exit 2
fi
`;
    const result = spawnSync('bash', ['-c', harness], {
      env: { ...process.env, TMPDIR: scratch, TEST_MAIN_HOME: home, TEST_RECORD: record, TEST_IMPORT_RECORD: importRecord,
        TEST_RM_OBSERVED: rmObserved, TEST_SCENARIO: scenario, LOCUS_GRAPH_PASSPHRASE: 'graph-test' },
      encoding: 'utf8',
    });
    assert.equal(result.status, scenario.endsWith('failure') || scenario === 'relocated' ? 1 : 0);
    assert.equal(result.signal, null);
    if (scenario === 'cleanup-failure') {
      assert.match(result.stderr, /graph import temporary home was not removed/);
    } else if (scenario === 'relocated') {
      assert.match(result.stderr, /GRAPH_HOME was not created directly beneath TMPDIR/);
      assert.equal(existsSync(home), false, 'relocated graph home survived exit cleanup');
    } else if (scenario === 'init-failure' || scenario === 'import-failure') {
      assert.match(result.stderr, /graph import failed/);
    }
    assert.deepEqual(readdirSync(scratch), ['keep']);
    assert.equal(existsSync(rmObserved), scenario === 'success',
      'only the success path must route the populated graph home through rm');
    if (scenario === 'same-home' || scenario === 'relocated') {
      assert.throws(() => readFileSync(record), { code: 'ENOENT' });
    } else {
      const graphHome = readFileSync(record, 'utf8');
      assert.equal(dirname(graphHome), scratch);
      assert.match(graphHome, /\/locus-graph-imp\.[^/]+$/);
      assert.throws(() => lstatSync(graphHome), { code: 'ENOENT' }, 'graph home survived exit cleanup');
      if (scenario === 'success') {
        assert.equal(readFileSync(rmObserved, 'utf8'), graphHome,
          'rm was observed removing a different directory');
      }
      if (scenario === 'init-failure') {
        assert.throws(() => readFileSync(importRecord), { code: 'ENOENT' });
      } else {
        assert.equal(readFileSync(importRecord, 'utf8'), graphHome, 'fresh-home import was not reached');
      }
    }
  });
}

// A stubbed mktemp records its template, proving root selection (TMPDIR, else /tmp)
// independently of where the real mktemp happens to place the directory.
for (const [name, useTmpdir] of [['TMPDIR set', true], ['TMPDIR unset', false]]) {
  test(`graph home mktemp template is rooted by the temporary root (${name})`, (t) => {
    const fixture = mkdtempSync(join(tmpdir(), 'locus-graph-stub-'));
    t.after(() => rmSync(fixture, { recursive: true, force: true }));
    const scratch = join(fixture, 'temp with spaces');
    const templates = join(fixture, 'templates');
    mkdirSync(scratch);
    const harness = `#!/bin/bash
set -uo pipefail
GRAPH_HOME=""
log() { :; }
ok() { :; }
die() { printf '%s\\n' "$*" >&2; exit 1; }
mktemp() { printf '%s\\n' "$2" >> "$TEST_TEMPLATES"; command mktemp "$@"; }
locus() { return 1; }
GRAPH_HOME="$(mktemp -d "\${TMPDIR:-/tmp}/locus-graph-imp.XXXXXX")"
[[ -d "$GRAPH_HOME" ]] && command rm -rf "$GRAPH_HOME"
`;
    const env = { ...process.env, TEST_TEMPLATES: templates };
    if (useTmpdir) env.TMPDIR = scratch; else delete env.TMPDIR;
    const result = spawnSync('bash', ['-c', harness], { env, encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(readFileSync(templates, 'utf8').trim(),
      `${useTmpdir ? scratch : '/tmp'}/locus-graph-imp.XXXXXX`);
  });
}

test('e2e.sh graph home uses the same TMPDIR-or-/tmp template as the stubbed probe', () => {
  assert.ok(createGraphHome.includes('GRAPH_HOME="$(mktemp -d "${TMPDIR:-/tmp}/locus-graph-imp.XXXXXX")"'));
});

// Runs the real graph section (not a copy of its mktemp line) with a stubbed
// mktemp that records its template, so the configured TMPDIR is proven to be used.
test('e2e.sh graph section passes the configured TMPDIR to a stubbed mktemp', (t) => {
  const fixture = mkdtempSync(join(tmpdir(), 'locus-graph-real-'));
  t.after(() => rmSync(fixture, { recursive: true, force: true }));
  const scratch = join(fixture, 'temp with spaces');
  const templates = join(fixture, 'templates');
  mkdirSync(scratch);
  mkdirSync(join(fixture, 'main-home'));
  const harness = `#!/bin/bash
set -euo pipefail
export LOCUS_HOME="$TEST_MAIN_HOME"
GRAPH_HOME="" WS_DIR="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
log() { :; }
ok() { :; }
skip() { exit 90; }
die() { printf '%s\\n' "$*" >&2; exit 1; }
has_cmd() { return 0; }
mktemp() { printf '%s\\n' "$2" >> "$TEST_TEMPLATES"; command mktemp "$@"; }
locus() {
  case "$1:\${2:-}" in
    graph:export) printf 'LOCUSGRAPH00' > "$4" ;;
    graph:import) [[ "$LOCUS_HOME" != "$TEST_MAIN_HOME" ]] ;;
    *) return 0 ;;
  esac
}
${cleanup}
${createGraphHome}
trap 'e2e_cleanup' EXIT
${graph}
`;
  const result = spawnSync('bash', ['-c', harness], {
    env: { ...process.env, TMPDIR: scratch, TEST_MAIN_HOME: join(fixture, 'main-home'),
      TEST_TEMPLATES: templates, LOCUS_GRAPH_PASSPHRASE: 'graph-test' },
    encoding: 'utf8',
  });
  assert.equal(result.status, 0, result.stderr);
  assert.ok(existsSync(templates), `mktemp stub was never called: ${result.stderr}`);
  const lines = readFileSync(templates, 'utf8').trim().split('\n');
  assert.ok(lines.includes(`${scratch}/locus-graph-imp.XXXXXX`), lines.join('\n'));
  assert.ok(lines.every((l) => l.startsWith(`${scratch}/`)), 'a template escaped TMPDIR');
});
