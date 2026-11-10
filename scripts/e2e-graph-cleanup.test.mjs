// Run: node --test scripts/e2e-graph-cleanup.test.mjs
// Failing-command contract after GRAPH_HOME is created: once scripts/e2e.sh
// has created a graph home, any later failing command must be reported and
// the temporary graph directory removed. The probes run the script's own
// create_graph_home, e2e_cleanup and repeated graph-home block verbatim,
// stubbing only mktemp/rm where noted, so a regression in any piece fails
// here rather than in a copy.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, lstatSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const source = readFileSync(join(root, 'scripts', 'e2e.sh'), 'utf8');

function piece(name) {
  const body = source.match(new RegExp(`^${name}\\(\\) \\{[\\s\\S]*?^\\}`, 'm'))?.[0];
  assert.ok(body, `e2e.sh must define ${name}`);
  return body;
}

// The unconditional repeated-creation block at the tail of section 16, taken
// from the script so its own guards, checks and ok line are the ones run.
function repeatedBlock() {
  const start = source.search(/^create_graph_home \|\| die/m);
  const endMarker = 'ok "two graph homes are distinct and live directly beneath the temp root"';
  const end = source.indexOf(endMarker, start);
  assert.ok(start >= 0 && end > start, 'e2e.sh must contain the repeated graph-home block');
  return source.slice(start, end + endMarker.length);
}

// Removes the trap's registry loop, leaving the general removal line intact.
function stripRegistryLoop(cleanup) {
  const mutated = cleanup.replace(
    /\n[ \t]*# Every graph home this run created[\s\S]*?done <<< "\$\{GRAPH_HOMES:-\}"\n/,
    '\n',
  );
  assert.notEqual(mutated, cleanup, 'the registry removal must be present to mutate');
  return mutated;
}

// Runs the real pieces beneath a controlled temporary root. okCount lets the
// mktemp stub fail every graph-home creation after the first okCount succeed;
// rmFail makes the block's explicit removal fail so only the trap can still
// remove the homes; omitRegistry disables the trap's registry loop.
function run(t, { okCount = 2, rmFail = false, omitRegistry = false } = {}) {
  const dir = mkdtempSync(join(tmpdir(), 'locus-e2e-graph-cleanup-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const scratch = join(dir, 'scratch root');
  const mainHome = join(dir, 'main-home');
  const created = join(dir, 'created');
  const mkCount = join(dir, 'mk-count');
  mkdirSync(scratch);
  mkdirSync(mainHome);
  writeFileSync(join(scratch, 'keep'), 'unrelated');

  let cleanup = piece('e2e_cleanup');
  if (omitRegistry) {
    cleanup = stripRegistryLoop(cleanup);
  }
  const shim = `
mktemp() {
  n="$(cat "$MK_COUNT" 2>/dev/null || printf 0)"
  n=$(( n + 1 ))
  printf '%s' "$n" > "$MK_COUNT"
  if [[ "$n" -le "$MK_OK" ]]; then
    created="$(command mktemp "$@")" || return 1
    printf '%s\\n' "$created" >> "$CREATED"
    printf '%s\\n' "$created"
  else
    return 1
  fi
}
${rmFail ? 'rm() {\n  if [[ "$#" -eq 3 && "$1" == -rf && "$2" != -- ]]; then return 0; fi\n  command rm "$@"\n}\n' : ''}`;
  const result = spawnSync('bash', ['-c', `set -euo pipefail
${cleanup}
${piece('create_graph_home')}
${shim}
log() { :; }
ok() { printf 'ok %s\\n' "$*"; }
die() { printf 'FAIL %s\\n' "$*" >&2; exit 1; }
LOCUS_HOME="$MAIN_HOME"
WS_DIR="" GRAPH_HOME="" GRAPH_HOMES="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
trap 'e2e_cleanup' EXIT
${repeatedBlock()}
`], {
    env: { ...process.env, TMPDIR: scratch, MAIN_HOME: mainHome, CREATED: created,
      MK_COUNT: mkCount, MK_OK: String(okCount), RM_FAIL: rmFail ? '1' : '0' },
    encoding: 'utf8',
  });
  const createdHomes = existsSync(created)
    ? readFileSync(created, 'utf8').trim().split('\n').filter((line) => line)
    : [];
  return { scratch, mainHome, createdHomes, result };
}

test('success removes both repeated graph homes and keeps unrelated data', (t) => {
  const { scratch, mainHome, createdHomes, result } = run(t, {});
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.signal, null);
  assert.match(result.stdout,
    /ok two graph homes are distinct and live directly beneath the temp root/);
  assert.equal(createdHomes.length, 2, 'both creations must succeed');
  for (const home of createdHomes) {
    assert.equal(dirname(home), scratch);
    assert.match(home, /\/locus-graph-imp\.[^/]+$/);
    assert.throws(() => lstatSync(home), { code: 'ENOENT' }, 'graph home must be removed');
  }
  assert.deepEqual(readdirSync(scratch), ['keep'], 'cleanup must preserve unrelated files');
  assert.ok(!existsSync(mainHome), 'the trap removes the main home too');
});

test('a failing second creation is reported and the first home is removed', (t) => {
  const { scratch, mainHome, createdHomes, result } = run(t, { okCount: 1 });
  assert.equal(result.status, 1, 'the failed creation must fail the suite');
  assert.equal(result.signal, null);
  assert.match(result.stderr, /FAIL could not create the temporary graph home/);
  assert.equal(createdHomes.length, 1);
  assert.throws(() => lstatSync(createdHomes[0]), { code: 'ENOENT' },
    'the first created home must be removed although a failed creation clears GRAPH_HOME');
  assert.deepEqual(readdirSync(scratch), ['keep'],
    'no locus-graph-imp.* directory may survive a failed creation');
  assert.ok(!existsSync(mainHome), 'the trap removes the main home too');
});

test('a failing first creation is reported and creates nothing', (t) => {
  const { scratch, mainHome, createdHomes, result } = run(t, { okCount: 0 });
  assert.equal(result.status, 1, 'the failed creation must fail the suite');
  assert.equal(result.signal, null);
  assert.match(result.stderr, /FAIL could not create the temporary graph home/);
  assert.equal(createdHomes.length, 0, 'a failed creation must not record a directory');
  assert.deepEqual(readdirSync(scratch), ['keep']);
  assert.ok(!existsSync(mainHome), 'the trap removes the main home too');
});

test('a failed removal is reported and the trap still removes the homes', (t) => {
  const { scratch, mainHome, createdHomes, result } = run(t, { rmFail: true });
  assert.equal(result.status, 1, 'the failed removal must fail the suite');
  assert.equal(result.signal, null);
  assert.match(result.stderr, /FAIL repeated graph homes were not removed/);
  assert.equal(createdHomes.length, 2);
  for (const home of createdHomes) {
    assert.throws(() => lstatSync(home), { code: 'ENOENT' },
      'the trap registry must remove the homes the explicit removal failed to remove');
  }
  assert.deepEqual(readdirSync(scratch), ['keep'],
    'no locus-graph-imp.* directory may survive a failed removal');
  assert.ok(!existsSync(mainHome), 'the trap removes the main home too');
});

test('the trap registry is the backup removal when the explicit removal fails', (t) => {
  // Mutated cleanup without the registry loop: the suite still reports the
  // failed removal, but the homes then survive — proving the registry, not the
  // positive path, is the removal this contract relies on.
  const { scratch, createdHomes, result } = run(t, { rmFail: true, omitRegistry: true });
  assert.equal(result.status, 1, 'the failed removal must still fail the suite');
  assert.match(result.stderr, /FAIL repeated graph homes were not removed/);
  assert.equal(createdHomes.length, 2);
  for (const home of createdHomes) {
    assert.ok(existsSync(home), 'without the registry the homes survive the failed removal');
  }
  assert.deepEqual(readdirSync(scratch).sort(),
    ['keep', ...createdHomes.map((home) => basename(home))].sort());
});
