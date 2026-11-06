// Run: node --test scripts/hub-integration-smoke-home-root.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { basename, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-integration-test.sh');

// Stub the prerequisites and stop at `locus init`, after confirming the
// exported LOCUS_HOME is a real directory directly beneath the isolated
// TMPDIR. The wrapped mktemp records what the real allocator creates; the
// injected init failure triggers the EXIT trap and its cleanup.
const harness = `
node() { :; }
jq() { :; }
cargo() { :; }
mktemp() {
  local path
  path="$(command mktemp "$@")" || return
  printf '%s\\n' "$path" >> "$RECORD"
  printf '%s\\n' "$path"
}
locus() {
  if [[ "$1" == init ]]; then
    [[ -d "$LOCUS_HOME" && "${'$'}{LOCUS_HOME%/*}" == "$TMPDIR" ]] || return 74
    printf '%s\\n' "$LOCUS_HOME" > "$OBSERVED"
    return 73
  fi
}
export -f node jq cargo mktemp locus
exec bash "$1"
`;

test('SMOKE_HOME is created directly beneath an isolated TMPDIR', (t) => {
  const scratch = mkdtempSync('/tmp/locus-home-root-');
  t.after(() => rmSync(scratch, { recursive: true, force: true }));
  const tmpdir = join(scratch, 'isolated root');
  const record = join(scratch, 'allocated');
  const observed = join(scratch, 'observed');
  mkdirSync(tmpdir);
  writeFileSync(join(tmpdir, 'keep'), 'sentinel');

  const result = spawnSync('bash', ['-c', harness, 'smoke-home-root', script], {
    env: { PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: tmpdir,
      RECORD: record, OBSERVED: observed },
    encoding: 'utf8',
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 73, 'the injected init stop must propagate after observing SMOKE_HOME');

  const allocated = readFileSync(record, 'utf8').trim().split('\n');
  assert.equal(allocated.length, 2, 'both temporary directories must be allocated before init');
  const home = readFileSync(observed, 'utf8').trim();
  assert.equal(home, allocated[0], 'LOCUS_HOME must be the first allocation, SMOKE_HOME');
  assert.equal(dirname(home), tmpdir, 'SMOKE_HOME must be created directly beneath TMPDIR');
  assert.match(basename(home), /^locus-hub-int\.[^/]+$/,
    'SMOKE_HOME must use the locus-hub-int.XXXXXX template');
  assert.equal(dirname(allocated[1]), tmpdir,
    'the smoke project must share the same temporary root');
  assert.match(basename(allocated[1]), /^locus-hub-int-proj\.[^/]+$/);
  assert.ok(result.stdout.includes(`== hub-integration-test LOCUS_HOME=${home} ==`),
    'the smoke run must report the home created beneath TMPDIR');
  for (const path of allocated) {
    assert.equal(existsSync(path), false, 'each allocated directory must be removed on exit');
  }
  assert.deepEqual(readdirSync(tmpdir), ['keep'], 'cleanup must leave the isolated root untouched');
  assert.equal(readFileSync(join(tmpdir, 'keep'), 'utf8'), 'sentinel');
});

test('a relative TMPDIR selects the home root and is safe to clean up', (t) => {
  const scratch = realpathSync(mkdtempSync('/tmp/locus-home-relative-'));
  t.after(() => rmSync(scratch, { recursive: true, force: true }));
  const relativeRoot = 'root [*] with spaces';
  const tmpdir = join(scratch, relativeRoot);
  const record = join(scratch, 'allocated');
  const observed = join(scratch, 'observed');
  mkdirSync(tmpdir);
  writeFileSync(join(tmpdir, 'keep'), 'sentinel');

  const result = spawnSync('bash', ['-c', harness, 'smoke-home-relative', script], {
    cwd: scratch,
    env: { PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: relativeRoot,
      RECORD: record, OBSERVED: observed },
    encoding: 'utf8',
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 73, 'init must observe a live home under the normalized root');
  const allocated = readFileSync(record, 'utf8').trim().split('\n');
  assert.equal(allocated.length, 2);
  const home = readFileSync(observed, 'utf8').trim();
  assert.equal(home, allocated[0]);
  assert.equal(dirname(home), tmpdir);
  assert.match(basename(home), /^locus-hub-int\.[^/]+$/);
  for (const path of allocated) {
    assert.equal(dirname(path), tmpdir);
    assert.equal(existsSync(path), false, 'cleanup must remove the allocated directory');
  }
  assert.deepEqual(readdirSync(tmpdir), ['keep']);
  assert.equal(readFileSync(join(tmpdir, 'keep'), 'utf8'), 'sentinel');
});

// With TMPDIR unset the script must fall back to /tmp, and SMOKE_HOME must be
// a live directory there while the run is in progress (not just a recorded path).
test('SMOKE_HOME is a live directory under /tmp when TMPDIR is unset', (t) => {
  const scratch = mkdtempSync('/tmp/locus-home-root-');
  t.after(() => rmSync(scratch, { recursive: true, force: true }));
  const record = join(scratch, 'allocated');
  const observed = join(scratch, 'observed');
  const unsetHarness = harness.replace('"${LOCUS_HOME%/*}" == "$TMPDIR"', '"${LOCUS_HOME%/*}" == /tmp');
  assert.notEqual(unsetHarness, harness, 'the parent-directory check must be rewritten to expect /tmp');

  const result = spawnSync('bash', ['-c', unsetHarness, 'smoke-home-default', script], {
    env: { PATH: process.env.PATH, HOME: process.env.HOME, RECORD: record, OBSERVED: observed },
    encoding: 'utf8',
  });

  assert.equal(result.error, undefined);
  assert.equal(result.status, 73, 'init must observe SMOKE_HOME as an existing directory directly under /tmp');
  const home = readFileSync(observed, 'utf8').trim();
  assert.equal(dirname(home), '/tmp');
  assert.match(basename(home), /^locus-hub-int\.[^/]+$/);
  assert.equal(existsSync(home), false, 'SMOKE_HOME must be removed on exit');
});
