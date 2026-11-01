// Run: node --test scripts/hub-smoke-error-failure.test.mjs
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-smoke.sh');
const realMktemp = execFileSync('which', ['mktemp'], { encoding: 'utf8' }).trim();

function smokeCommandGuard(root) {
  const guard = join(root, 'guard.sh');
  const marker = join(root, 'smoke-command-ran');
  writeFileSync(guard, `locus() { : > "$SMOKE_COMMAND_MARKER"; return 1; }
cargo() { : > "$SMOKE_COMMAND_MARKER"; return 1; }
jq() { : > "$SMOKE_COMMAND_MARKER"; return 1; }
`);
  return { BASH_ENV: guard, SMOKE_COMMAND_MARKER: marker };
}

test('failure creating SMOKE_ERR recursively cleans both previously created directories', (t) => {
  const root = mkdtempSync('/tmp/lhs-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const bin = join(root, 'bin');
  const scratch = join(root, 'temporary resources with spaces');
  const record = join(root, 'created');
  const failureObserved = join(root, 'error-file-allocation-failed');
  mkdirSync(bin);
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'sentinel');
  const guard = smokeCommandGuard(root);
  writeFileSync(join(bin, 'mktemp'), `#!/usr/bin/env bash
case "$*" in
  "$TMPDIR"/locus-hub-smoke-err.XXXXXX)
    count=0
    while IFS= read -r path; do
      [[ -f "$path/nested/payload" ]] || exit 72
      count=$((count + 1))
    done < "$MKTEMP_RECORD"
    [[ $count -eq 2 ]] || exit 72
    : > "$FAILURE_OBSERVED"
    exit 1 ;;
esac
path="$("$REAL_MKTEMP" "$@")" || exit
[[ -d "$path" ]] || exit 1
mkdir "$path/nested"
printf 'fixture\\n' > "$path/nested/payload"
printf '%s\\n' "$path" >> "$MKTEMP_RECORD"
printf '%s\\n' "$path"
`, { mode: 0o755 });
  const result = spawnSync('bash', [script], {
    env: { ...process.env, ...guard, TMPDIR: scratch, PATH: `${bin}:${process.env.PATH}`,
      REAL_MKTEMP: realMktemp, MKTEMP_RECORD: record, FAILURE_OBSERVED: failureObserved },
    encoding: 'utf8',
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /error: hub-smoke could not create temporary error file/);
  assert.equal(result.stdout, '');
  assert.equal(existsSync(guard.SMOKE_COMMAND_MARKER), false);
  assert.ok(existsSync(failureObserved), 'the error-file allocation must fail after both populated directories exist');
  const paths = readFileSync(record, 'utf8').trim().split('\n');
  assert.equal(paths.length, 2);
  for (const [index, prefix] of ['locus-hub-smoke.', 'locus-hub-proj.'].entries()) {
    assert.equal(dirname(paths[index]), scratch);
    assert.ok(paths[index].slice(scratch.length + 1).startsWith(prefix));
    assert.equal(existsSync(paths[index]), false, `${prefix} must be removed`);
  }
  assert.deepEqual(readdirSync(scratch), ['keep']);
  assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'sentinel');
});

