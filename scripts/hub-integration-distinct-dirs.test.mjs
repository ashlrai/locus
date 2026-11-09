// Run: node --test scripts/hub-integration-distinct-dirs.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync } from 'node:fs';
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


test('two invocations create distinct SMOKE_PROJ directories under the selected root and clean up', (t) => {
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
    assert.equal(dirname(proj), tmp, `run ${n}: project must be directly under TMPDIR`);
    assert.match(proj.slice(tmp.length + 1), /^locus-hub-int-proj\.[^/]+$/);
    assert.equal(existsSync(proj), false, `run ${n}: project must be removed on exit`);
    return { home, proj };
  });

  const all = runs.flatMap(({ home, proj }) => [home, proj]);
  assert.equal(new Set(all).size, 4, 'no directory name may repeat across invocations');
  assert.notEqual(runs[0].home, runs[1].home, 'SMOKE_HOME must differ between invocations');
  assert.notEqual(runs[0].proj, runs[1].proj, 'SMOKE_PROJ must differ between invocations');
  for (const p of all) assert.equal(existsSync(p), false, `removed on exit: ${p}`);
  assert.deepEqual(readdirSync(tmp), [], 'neither invocation may leave anything behind');
});
