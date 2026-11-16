// Run: node --test scripts/hub-smoke-home-repeated.test.mjs
// The SMOKE_HOME creation statement must yield a fresh directory on every
// call. hub-smoke-home-distinct.test.mjs covers separate script invocations;
// this test repeats the script's own creation statement twice in one shell,
// with the first directory still live, so a fixed-path or clobbering rewrite
// would collide with — or delete — the first directory instead of producing
// a second.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { basename, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-smoke.sh');
const source = readFileSync(script, 'utf8');

test('repeated SMOKE_HOME creation yields two distinct directories under a spaced TMPDIR', (t) => {
  const root = mkdtempSync('/tmp/lhs-home-repeat-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'scratch dir');
  const observed = join(root, 'observed');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  mkdirSync(join(scratch, 'locus-hub-smoke.unrelated'));

  const creation = source.match(/if ! (SMOKE_HOME="\$\(mktemp [^;]+?\)"); then/)?.[1];
  assert.ok(creation, 'missing SMOKE_HOME creation statement');

  // Replay the exact creation statement twice in one shell. A marker file is
  // left inside the first directory so a second creation that removed or
  // reused the same path would be caught, not just a reused name.
  const result = spawnSync('bash', ['-c', `set -euo pipefail
TMPDIR="$1"
OBSERVED="$2"
SMOKE_HOME=""
${creation}
first="$SMOKE_HOME"
printf 'recorded\\n' > "$first/marker"
${creation}
second="$SMOKE_HOME"
[[ "$first" != "$second" && ! "$first" -ef "$second" ]] || exit 1
[[ -d "$first" && ! -L "$first" && "$(cat "$first/marker")" == recorded ]] || exit 1
[[ -d "$second" && ! -L "$second" && -z "$(ls -A "$second")" ]] || exit 1
[[ "\${first%/*}" == "$TMPDIR" && "\${second%/*}" == "$TMPDIR" ]] || exit 1
[[ "\${first##*/}" == locus-hub-smoke.* && "\${second##*/}" == locus-hub-smoke.* ]] || exit 1
printf '%s\\n%s\\n' "$first" "$second" > "$OBSERVED"
`, 'smoke-home-repeat', scratch, observed], { encoding: 'utf8' });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 0, `stderr:\n${result.stderr}`);

  const [first, second] = readFileSync(observed, 'utf8').trim().split('\n');
  assert.notEqual(first, second, 'the second creation must not reuse the first path');
  for (const dir of [first, second]) {
    assert.equal(dirname(dir), scratch, 'each home is created directly beneath the spaced TMPDIR');
    assert.match(basename(dir), /^locus-hub-smoke\.[A-Za-z0-9]{6}$/);
    assert.ok(statSync(dir).isDirectory());
    assert.equal(statSync(dir).mode & 0o777, 0o700, 'mktemp -d homes must be private (0700)');
  }
  assert.equal(readFileSync(join(first, 'marker'), 'utf8'), 'recorded\n',
    'the second creation must not clobber the first directory');
  assert.deepEqual(readdirSync(second), [], 'the second directory must be created empty');
  // Both directories live outside the script's cleanup trap; the test removes them.
  rmSync(first, { recursive: true });
  rmSync(second, { recursive: true });
  assert.deepEqual(readdirSync(scratch).sort(), ['keep', 'locus-hub-smoke.unrelated']);
});
