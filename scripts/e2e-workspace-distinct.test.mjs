// Run: node --test scripts/e2e-workspace-distinct.test.mjs
// Every create_workspace call in scripts/e2e.sh must allocate a fresh WS_DIR
// through mktemp -d. A fixed or recycled path would let one rehearsal or probe
// observe another workspace's allowlist fixture — and cleanup of one could
// delete the other's live directory. These tests replay the script's own
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

function createWorkspace() {
  const create = source.match(/^create_workspace\(\) \{[\s\S]*?^\}/m)?.[0];
  assert.ok(create, 'e2e.sh must define create_workspace');
  return create;
}

function e2eCleanup() {
  const cleanup = source.match(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m)?.[0];
  assert.ok(cleanup, 'e2e.sh must define e2e_cleanup');
  return cleanup;
}

// Replay the exact allocation statement twice in one shell. A marker file is
// left inside the first directory so a second allocation that removed or
// reused the same path would be caught, not just a reused name.
function repeatAllocations(t, scratchName) {
  const root = mkdtempSync(join(tmpdir(), 'locus-e2e-ws-distinct-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, scratchName);
  const observed = join(root, 'observed');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  mkdirSync(join(scratch, 'locus-ws.unrelated'));

  const result = spawnSync('bash', ['-c', `set -euo pipefail
${createWorkspace()}
WS_DIR=""
create_workspace
first="$WS_DIR"
printf 'recorded\\n' > "$first/marker"
create_workspace
second="$WS_DIR"
[[ "$first" != "$second" && ! "$first" -ef "$second" ]] || exit 1
[[ -d "$first" && ! -L "$first" && "$(cat "$first/marker")" == recorded ]] || exit 1
[[ -d "$second" && ! -L "$second" && -z "$(ls -A "$second")" ]] || exit 1
[[ "\${first%/*}" == "$TMPDIR" && "\${second%/*}" == "$TMPDIR" ]] || exit 1
[[ "\${first##*/}" == locus-ws.* && "\${second##*/}" == locus-ws.* ]] || exit 1
printf '%s\\n%s\\n' "$first" "$second" > "$OBSERVED"
`], { env: { ...process.env, TMPDIR: scratch, OBSERVED: observed }, encoding: 'utf8' });
  return { result, scratch, observed };
}

for (const [mode, scratchName] of [['plain', 'scratch'], ['spaced', 'scratch dir [*]']]) {
  test(`repeated create_workspace calls allocate distinct workspaces under a ${mode} temporary root`, (t) => {
    const { result, scratch, observed } = repeatAllocations(t, scratchName);

    assert.equal(result.error, undefined);
    assert.equal(result.signal, null);
    assert.equal(result.status, 0, `stderr:\n${result.stderr}`);

    const [first, second] = readFileSync(observed, 'utf8').trim().split('\n');
    assert.notEqual(first, second, 'the second allocation must not reuse the first path');
    for (const dir of [first, second]) {
      assert.equal(dirname(dir), scratch, 'each workspace is created directly beneath the TMPDIR');
      assert.match(basename(dir), /^locus-ws\.[A-Za-z0-9]{6}$/);
      assert.ok(statSync(dir).isDirectory());
      assert.equal(statSync(dir).mode & 0o777, 0o700, 'mktemp -d workspaces must be private (0700)');
    }
    assert.equal(readFileSync(join(first, 'marker'), 'utf8'), 'recorded\n',
      'the second allocation must not clobber the first workspace');
    assert.deepEqual(readdirSync(second), [], 'the second workspace must be created empty');
    // Both directories live outside the script's cleanup trap; the test removes them.
    rmSync(first, { recursive: true });
    rmSync(second, { recursive: true });
    assert.deepEqual(readdirSync(scratch).sort(), ['keep', 'locus-ws.unrelated']);
  });
}

// Two shells racing the same temporary root must also never collide: separate
// allocations are the only thing keeping concurrent rehearsals off each
// other's fixtures.
test('concurrent create_workspace calls in separate shells allocate distinct workspaces', { timeout: 30_000 }, async (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-e2e-ws-distinct-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'scratch');
  mkdirSync(scratch);

  const allocate = (index) => new Promise((resolve, reject) => {
    const record = join(root, `allocated-${index}`);
    const child = spawn('bash', ['-c', `set -euo pipefail
${createWorkspace()}
WS_DIR=""
create_workspace
[[ -d "$WS_DIR" && "\${WS_DIR%/*}" == "$TMPDIR" && "\${WS_DIR##*/}" == locus-ws.* ]] || exit 1
printf '%s' "$WS_DIR" > "$RECORD"
printf 'owned by %s' "$INDEX" > "$WS_DIR/marker"
`, 'e2e-ws-allocate'], {
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
    assert.match(basename(dir), /^locus-ws\.[A-Za-z0-9]{6}$/);
    assert.ok(statSync(dir).isDirectory());
    assert.equal(readFileSync(join(dir, 'marker'), 'utf8'), `owned by ${index}`,
      'each shell must see only its own workspace');
    rmSync(dir, { recursive: true });
  }
  assert.deepEqual(readdirSync(scratch), []);
});

// The exit trap removes the allocation WS_DIR currently names. A rehearsal
// that allocates twice leaves the earlier directory for the caller to remove;
// the trap must not reach back and delete the stale path — the earlier
// workspace has to stay live and intact for whoever still uses it.
test('exit cleanup removes the live WS_DIR allocation and spares an earlier distinct allocation', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-e2e-ws-distinct-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'custom tmp');
  const home = join(root, 'subshell home');
  const observed = join(root, 'observed');
  mkdirSync(scratch);
  mkdirSync(home);
  writeFileSync(join(scratch, 'keep'), 'unrelated');

  const result = spawnSync('bash', ['-c', `set -euo pipefail
${e2eCleanup()}
${createWorkspace()}
LOCUS_HOME="$TEST_HOME"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
trap 'e2e_cleanup' EXIT
create_workspace
first="$WS_DIR"
printf 'first\\n' > "$first/marker"
create_workspace
second="$WS_DIR"
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
    assert.match(basename(dir), /^locus-ws\.[A-Za-z0-9]{6}$/);
  }
  assert.throws(() => lstatSync(second), { code: 'ENOENT' },
    'the live allocation must be removed by the exit trap');
  assert.throws(() => lstatSync(home), { code: 'ENOENT' },
    'the subshell home must be removed by the exit trap');
  assert.equal(readFileSync(join(first, 'marker'), 'utf8'), 'first\n',
    'cleaning the live allocation must not touch the earlier distinct workspace');
  rmSync(first, { recursive: true });
  assert.deepEqual(readdirSync(scratch), ['keep']);
});

test('WS_DIR is only ever assigned through create_workspace', () => {
  const lines = source.split('\n');
  const assigns = lines.filter((l) => /^\s*WS_DIR=("\$\(|\$\()/.test(l));
  assert.equal(assigns.length, 1, `unexpected WS_DIR creations: ${assigns.join(' | ')}`);
  assert.match(assigns[0], /mktemp -d "\$\{TMPDIR:-\/tmp\}\/locus-ws\.XXXXXX"/);
  assert.ok(source.includes('"${WS_DIR:-}"'), 'WS_DIR must be covered by exit cleanup');
});
