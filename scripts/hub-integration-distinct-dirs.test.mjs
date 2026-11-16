// Run: node --test scripts/hub-integration-distinct-dirs.test.mjs
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const script = join(root, 'scripts/hub-integration-test.sh');

// Record SMOKE_HOME and SMOKE_PROJ as seen at `locus init`, then abort; the
// EXIT trap still runs.
const harness = `
node() { :; }
jq() { :; }
cargo() { :; }
locus() {
  if [[ "$1" == init ]]; then
    [[ -d "$SMOKE_HOME" && -d "$SMOKE_PROJ" ]] || return 74
    printf '%s\\n' "$SMOKE_HOME" "$SMOKE_PROJ" > "$RECORD"
    return 73
  fi
}
export -f node jq cargo locus
exec bash "$1"
`;

test('SMOKE_HOME and SMOKE_PROJ are two distinct, non-nested directories under TMPDIR', (t) => {
  const scratch = mkdtempSync('/tmp/locus-distinct-test-');
  t.after(() => rmSync(scratch, { recursive: true, force: true }));
  const tmp = join(scratch, 'tmp');
  const record = join(scratch, 'observed');
  mkdirSync(tmp);

  const result = spawnSync('bash', ['-c', harness, 'distinct-test', script], {
    env: { PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: tmp, RECORD: record },
    encoding: 'utf8',
  });
  assert.equal(result.error, undefined);
  assert.equal(result.status, 73, 'both directories must exist when init runs');
  const [home, proj] = readFileSync(record, 'utf8').trim().split('\n');
  assert.notEqual(home, proj);
  assert.equal(dirname(home), tmp);
  assert.equal(dirname(proj), tmp);
  assert.ok(!proj.startsWith(`${home}/`) && !home.startsWith(`${proj}/`), 'must not be nested');
  for (const p of [home, proj]) assert.equal(existsSync(p), false, 'removed on exit');
});

test('both directories are created beneath a TMPDIR containing spaces', (t) => {
  const scratch = mkdtempSync('/tmp/locus-distinct-test-');
  t.after(() => rmSync(scratch, { recursive: true, force: true }));
  const tmp = join(scratch, 'requested location with spaces');
  const record = join(scratch, 'observed');
  mkdirSync(tmp);

  const result = spawnSync('bash', ['-c', harness, 'spaced-test', script], {
    env: { PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: tmp, RECORD: record },
    encoding: 'utf8',
  });
  assert.equal(result.error, undefined);
  assert.equal(result.status, 73, 'both directories must exist when init runs');
  const paths = readFileSync(record, 'utf8').trim().split('\n');
  assert.equal(paths.length, 2);
  assert.notEqual(paths[0], paths[1]);
  assert.match(paths[0], /\/locus-hub-int\.[^/]+$/);
  assert.match(paths[1], /\/locus-hub-int-proj\.[^/]+$/);
  for (const path of paths) {
    assert.equal(dirname(path), tmp, 'created directly beneath the requested location');
    assert.equal(existsSync(path), false, 'removed on exit');
  }
  assert.deepEqual(readdirSync(tmp), []);
});


test('spaced temporary paths reach allocation and cleanup as single arguments', (t) => {
  const scratch = mkdtempSync('/tmp/locus-arguments-test-');
  t.after(() => rmSync(scratch, { recursive: true, force: true }));
  const tmp = join(scratch, 'requested location with spaces');
  const record = join(scratch, 'observed');
  const allocations = join(scratch, 'allocations');
  const removals = join(scratch, 'removals');
  mkdirSync(tmp);
  writeFileSync(join(tmp, 'keep'), 'untouched');
  const recordingHarness = `
mktemp() { printf '%s\\0' "$#" "$@" >> "$ALLOCATIONS"; command mktemp "$@"; }
rm() { printf '%s\\0' "$#" "$@" >> "$REMOVALS"; command rm "$@"; }
export -f mktemp rm
${harness}`;
  const result = spawnSync('bash', ['-c', recordingHarness, 'spaced-arguments', script], {
    env: { PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: tmp, RECORD: record,
      ALLOCATIONS: allocations, REMOVALS: removals },
    encoding: 'utf8',
  });
  assert.equal(result.error, undefined);
  assert.equal(result.status, 73);
  const args = (file) => readFileSync(file, 'utf8').split('\0').slice(0, -1);
  assert.deepEqual(args(allocations), [
    '2', '-d', `${tmp}/locus-hub-int.XXXXXX`,
    '2', '-d', `${tmp}/locus-hub-int-proj.XXXXXX`,
  ]);
  const paths = readFileSync(record, 'utf8').trim().split('\n');
  assert.deepEqual(args(removals), [
    '3', '-f', '--', `${tmp}/.locus-hub-int-scan.${result.pid}.*`,
    '4', '-rf', '--', ...paths,
  ]);
  assert.deepEqual(readdirSync(tmp), ['keep']);
  assert.equal(readFileSync(join(tmp, 'keep'), 'utf8'), 'untouched');
});

test('two invocations create distinct SMOKE_HOME directories and each cleans up its own', (t) => {
  const scratch = mkdtempSync('/tmp/locus-distinct-test-');
  t.after(() => rmSync(scratch, { recursive: true, force: true }));
  const tmp = join(scratch, 'tmp');
  mkdirSync(tmp);

  const runs = [1, 2].map((n) => {
    const record = join(scratch, `observed-${n}`);
    const result = spawnSync('bash', ['-c', harness, 'distinct-test', script], {
      env: { PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: tmp, RECORD: record },
      encoding: 'utf8',
    });
    assert.equal(result.error, undefined);
    assert.equal(result.status, 73, `run ${n}: both directories must exist when init runs`);
    const [home, proj] = readFileSync(record, 'utf8').trim().split('\n');
    return { home, proj };
  });

  const all = runs.flatMap(({ home, proj }) => [home, proj]);
  assert.equal(new Set(all).size, 4, 'no directory name may repeat across invocations');
  assert.notEqual(runs[0].home, runs[1].home, 'SMOKE_HOME must differ between invocations');
  for (const p of all) assert.equal(existsSync(p), false, `removed on exit: ${p}`);
  assert.deepEqual(readdirSync(tmp), [], 'neither invocation may leave anything behind');
});

test('concurrent invocations sharing one TMPDIR never collide on SMOKE_HOME', async (t) => {
  const scratch = mkdtempSync('/tmp/locus-distinct-test-');
  t.after(() => rmSync(scratch, { recursive: true, force: true }));
  const tmp = join(scratch, 'tmp');
  mkdirSync(tmp);

  const runs = await Promise.all(
    [1, 2, 3, 4].map(
      (n) =>
        new Promise((resolve, reject) => {
          const record = join(scratch, `observed-${n}`);
          const child = spawn('bash', ['-c', harness, 'distinct-test', script], {
            env: { PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: tmp, RECORD: record },
            stdio: 'ignore',
          });
          child.on('error', reject);
          child.on('close', (status) => {
            assert.equal(status, 73, `run ${n}: both directories must exist when init runs`);
            resolve(readFileSync(record, 'utf8').trim().split('\n'));
          });
        }),
    ),
  );

  const all = runs.flat();
  assert.equal(new Set(all).size, 8, 'every SMOKE_HOME and SMOKE_PROJ must be unique');
  for (const p of all) assert.equal(dirname(p), tmp);
  assert.deepEqual(readdirSync(tmp), [], 'no invocation may leave anything behind');
});
