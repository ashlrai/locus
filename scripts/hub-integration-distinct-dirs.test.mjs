// Run: node --test scripts/hub-integration-distinct-dirs.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
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

