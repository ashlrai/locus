// Run: node --test scripts/e2e-home-repeated.test.mjs
// Every create_locus_home call in scripts/e2e.sh must allocate a fresh
// LOCUS_HOME through mktemp -d. A fixed or recycled path would let one
// rehearsal observe another home's bindings and seal state — and cleanup of
// one could delete the other's live home. These tests replay the script's own
// allocation function twice under one temporary root and assert the second
// call yields a distinct, live directory without disturbing the first.
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { lstatSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'e2e.sh');
const source = readFileSync(script, 'utf8');

function createLocusHome() {
  const create = source.match(/^create_locus_home\(\) \{[\s\S]*?^\}/m)?.[0];
  assert.ok(create, 'e2e.sh must define create_locus_home');
  return create;
}

function e2eCleanup() {
  const cleanup = source.match(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m)?.[0];
  assert.ok(cleanup, 'e2e.sh must define e2e_cleanup');
  return cleanup;
}

// Replay the exact allocation call twice in one shell. A marker file is left
// inside the first directory so a second allocation that removed or reused
// the same path would be caught, not just a reused name.
function repeatAllocations(t, scratchName) {
  const root = mkdtempSync(join(tmpdir(), 'locus-e2e-home-repeat-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, scratchName);
  const observed = join(root, 'observed');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  mkdirSync(join(scratch, 'locus-e2e.unrelated'));

  const result = spawnSync('bash', ['-c', `set -euo pipefail
${createLocusHome()}
export LOCUS_HOME
LOCUS_HOME=""
create_locus_home
first="$LOCUS_HOME"
printf 'recorded\\n' > "$first/marker"
create_locus_home
second="$LOCUS_HOME"
[[ "$first" != "$second" && ! "$first" -ef "$second" ]] || exit 1
[[ -d "$first" && ! -L "$first" && "$(cat "$first/marker")" == recorded ]] || exit 1
[[ -d "$second" && ! -L "$second" && -z "$(ls -A "$second")" ]] || exit 1
[[ "\${first%/*}" == "$TMPDIR" && "\${second%/*}" == "$TMPDIR" ]] || exit 1
[[ "\${first##*/}" == locus-e2e.* && "\${second##*/}" == locus-e2e.* ]] || exit 1
printf '%s\\n%s\\n' "$first" "$second" > "$OBSERVED"
`], { env: { ...process.env, TMPDIR: scratch, OBSERVED: observed }, encoding: 'utf8' });
  return { result, scratch, observed };
}

for (const [mode, scratchName] of [['plain', 'scratch'], ['spaced', 'scratch dir [*]']]) {
  test(`repeated create_locus_home calls allocate distinct homes under a ${mode} temporary root`, (t) => {
    const { result, scratch, observed } = repeatAllocations(t, scratchName);

    assert.equal(result.error, undefined);
    assert.equal(result.signal, null);
    assert.equal(result.status, 0, `stderr:\n${result.stderr}`);

    const [first, second] = readFileSync(observed, 'utf8').trim().split('\n');
    assert.notEqual(first, second, 'the second allocation must not reuse the first path');
    for (const dir of [first, second]) {
      assert.equal(dirname(dir), scratch, 'each home is created directly beneath the TMPDIR');
      assert.match(basename(dir), /^locus-e2e\.[A-Za-z0-9]{6}$/);
      assert.ok(statSync(dir).isDirectory());
      assert.equal(statSync(dir).mode & 0o777, 0o700, 'mktemp -d homes must be private (0700)');
    }
    assert.equal(readFileSync(join(first, 'marker'), 'utf8'), 'recorded\n',
      'the second allocation must not clobber the first home');
    assert.deepEqual(readdirSync(second), [], 'the second home must be created empty');
    // Both directories live outside the script's cleanup trap; the test removes them.
    rmSync(first, { recursive: true });
    rmSync(second, { recursive: true });
    assert.deepEqual(readdirSync(scratch).sort(), ['keep', 'locus-e2e.unrelated']);
  });
}

// Two shells racing the same temporary root must also never collide: separate
// allocations are the only thing keeping concurrent rehearsals off each
// other's fixtures.
test('concurrent create_locus_home calls in separate shells allocate distinct homes', { timeout: 30_000 }, async (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-e2e-home-repeat-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'scratch');
  mkdirSync(scratch);

  const allocate = (index) => new Promise((resolve, reject) => {
    const record = join(root, `allocated-${index}`);
    const child = spawn('bash', ['-c', `set -euo pipefail
${createLocusHome()}
export LOCUS_HOME
LOCUS_HOME=""
create_locus_home
[[ -d "$LOCUS_HOME" && "\${LOCUS_HOME%/*}" == "$TMPDIR" && "\${LOCUS_HOME##*/}" == locus-e2e.* ]] || exit 1
printf '%s' "$LOCUS_HOME" > "$RECORD"
printf 'owned by %s' "$INDEX" > "$LOCUS_HOME/marker"
`, 'e2e-home-allocate'], {
      env: { ...process.env, TMPDIR: scratch, RECORD: record, INDEX: String(index) },
    });
    t.after(() => { if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL'); });
    let stderr = '';
    child.stderr.on('data', (chunk) => { stderr += chunk; });
    child.once('error', reject);
    child.once('exit', (code, signal) => resolve({ code, signal, stderr, record, index }));
  });

  const runs = await Promise.all([allocate(0), allocate(1)]);
  const allocated = [];
  for (const run of runs) {
    assert.equal(run.signal, null);
    assert.equal(run.code, 0, `run ${run.index} stderr:\n${run.stderr}`);
    allocated.push(readFileSync(run.record, 'utf8'));
  }
  const [first, second] = allocated;
  assert.notEqual(first, second, 'concurrent allocations must not collide');
  for (const [index, dir] of allocated.entries()) {
    assert.equal(dirname(dir), scratch);
    assert.match(basename(dir), /^locus-e2e\.[A-Za-z0-9]{6}$/);
    assert.ok(statSync(dir).isDirectory());
    assert.equal(readFileSync(join(dir, 'marker'), 'utf8'), `owned by ${index}`,
      'each shell must see only its own home');
    rmSync(dir, { recursive: true });
  }
  assert.deepEqual(readdirSync(scratch), []);
});

// The exit trap removes the allocation LOCUS_HOME currently names. A
// rehearsal that allocates twice leaves the earlier directory for the caller
// to remove; the trap must not reach back and delete the stale path — the
// earlier home has to stay live and intact for whoever still uses it.
test('exit cleanup removes the live LOCUS_HOME allocation and spares an earlier distinct allocation', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-e2e-home-repeat-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'custom tmp');
  const home = join(root, 'previous home');
  const observed = join(root, 'observed');
  mkdirSync(scratch);
  mkdirSync(home);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  writeFileSync(join(home, 'marker'), 'previous\n');

  const result = spawnSync('bash', ['-c', `set -euo pipefail
${e2eCleanup()}
${createLocusHome()}
export LOCUS_HOME
LOCUS_HOME="$TEST_HOME"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
trap 'e2e_cleanup' EXIT
create_locus_home
first="$LOCUS_HOME"
printf 'first\\n' > "$first/marker"
create_locus_home
second="$LOCUS_HOME"
printf 'second\\n' > "$second/marker"
[[ "$first" != "$second" && ! "$first" -ef "$second" ]] || exit 71
printf '%s\\n%s\\n' "$first" "$second" > "$OBSERVED"
`], { env: { ...process.env, TMPDIR: scratch, TEST_HOME: home, OBSERVED: observed }, encoding: 'utf8' });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 0, `stderr:\n${result.stderr}`);

  const [first, second] = readFileSync(observed, 'utf8').trim().split('\n');
  assert.notEqual(first, second);
  for (const dir of [first, second]) {
    assert.equal(dirname(dir), scratch);
    assert.match(basename(dir), /^locus-e2e\.[A-Za-z0-9]{6}$/);
  }
  assert.throws(() => lstatSync(second), { code: 'ENOENT' },
    'the live allocation must be removed by the exit trap');
  assert.equal(readFileSync(join(first, 'marker'), 'utf8'), 'first\n',
    'cleaning the live allocation must not touch the earlier distinct home');
  assert.equal(readFileSync(join(home, 'marker'), 'utf8'), 'previous\n',
    'the previously named home is abandoned by re-allocation, not removed');
  rmSync(first, { recursive: true });
  assert.deepEqual(readdirSync(scratch), ['keep']);
});

test('the suite home is only ever allocated through create_locus_home', () => {
  const lines = source.split('\n');
  const allocs = lines.filter((l) => /LOCUS_HOME="\$\(mktemp/.test(l));
  assert.equal(allocs.length, 1, `unexpected LOCUS_HOME allocations: ${allocs.join(' | ')}`);
  assert.match(allocs[0], /mktemp -d "\$\{TMPDIR:-\/tmp\}\/locus-e2e\.XXXXXX"/);
  assert.ok(/^export LOCUS_HOME$/m.test(source), 'the suite home must be exported for the binary');
  assert.ok(source.includes('rm -rf "$LOCUS_HOME"'), 'LOCUS_HOME must be covered by exit cleanup');
});
