// Run: node --test scripts/hub-smoke-err-repeated.test.mjs
// The SMOKE_ERR creation statement must yield a fresh file on every call.
// hub-smoke-err-distinct.test.mjs covers separate script invocations; this
// test repeats the script's own creation statement twice in one shell, with
// the first file still live, so a fixed-path or truncating rewrite would
// collide with — or clobber — the first file instead of producing a second.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { basename, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-smoke.sh');
const source = readFileSync(script, 'utf8');

test('repeated SMOKE_ERR creation yields two distinct files under a spaced TMPDIR', (t) => {
  const root = mkdtempSync('/tmp/lhs-err-repeat-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'scratch dir');
  const observed = join(root, 'observed');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  writeFileSync(join(scratch, 'locus-hub-smoke-err.unrelated'), 'sentinel');

  const creation = source.match(/if ! (SMOKE_ERR="\$\(mktemp [^;]+?\)"); then/)?.[1];
  assert.ok(creation, 'missing SMOKE_ERR creation statement');

  // Replay the exact creation statement twice in one shell. A sentinel is
  // written into the first file so a second creation that truncated or
  // overwrote the same path would be caught, not just a reused name.
  const result = spawnSync('bash', ['-c', `set -euo pipefail
TMPDIR="$1"
OBSERVED="$2"
SMOKE_ERR=""
${creation}
first="$SMOKE_ERR"
printf 'recorded\\n' > "$first"
${creation}
second="$SMOKE_ERR"
[[ "$first" != "$second" && ! "$first" -ef "$second" ]] || exit 1
[[ -f "$first" && ! -L "$first" && "$(cat "$first")" == recorded ]] || exit 1
[[ -f "$second" && ! -L "$second" && ! -s "$second" ]] || exit 1
[[ "\${first%/*}" == "$TMPDIR" && "\${second%/*}" == "$TMPDIR" ]] || exit 1
[[ "\${first##*/}" == locus-hub-smoke-err.* && "\${second##*/}" == locus-hub-smoke-err.* ]] || exit 1
printf '%s\\n%s\\n' "$first" "$second" > "$OBSERVED"
`, 'smoke-err-repeat', scratch, observed], { encoding: 'utf8' });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 0, `stderr:\n${result.stderr}`);

  const [first, second] = readFileSync(observed, 'utf8').trim().split('\n');
  assert.notEqual(first, second, 'the second creation must not reuse the first path');
  for (const file of [first, second]) {
    assert.equal(dirname(file), scratch, 'each error file is created directly beneath the spaced TMPDIR');
    assert.match(basename(file), /^locus-hub-smoke-err\.[A-Za-z0-9]{6}$/);
    assert.ok(statSync(file).isFile());
  }
  assert.equal(readFileSync(first, 'utf8'), 'recorded\n', 'the second creation must not clobber the first file');
  assert.equal(statSync(second).size, 0, 'the second file must be created empty');
  // Both files live outside the script's cleanup trap; the test removes them.
  rmSync(first);
  rmSync(second);
  assert.deepEqual(readdirSync(scratch).sort(), ['keep', 'locus-hub-smoke-err.unrelated']);
});
