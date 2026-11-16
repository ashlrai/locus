// Exercise the actual e2e walkthrough block without repeating the full suite.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const source = readFileSync(join(root, 'scripts/e2e.sh'), 'utf8');
const start = source.indexOf('# Replay the standalone sample');
const endMarker = 'ok "sample walkthrough exec succeeds without unset-variable warnings"';
const end = source.indexOf(endMarker, start);
assert.ok(start >= 0 && end > start, 'standalone walkthrough block must exist');
const block = source.slice(start, end + endMarker.length);

for (const [fault, expected] of [
  ['exit', 'sample walkthrough exec failed'],
  ['output', 'sample walkthrough exec produced unexpected output'],
  ['warning', 'sample walkthrough exec emitted an unset-variable warning'],
  ['none', null],
]) {
  test(`standalone sample walkthrough ${fault}`, (t) => {
    const dir = mkdtempSync(join(tmpdir(), 'locus-sample-walkthrough-'));
    t.after(() => rmSync(dir, { recursive: true, force: true }));
    const bin = join(dir, 'locus');
    const called = join(dir, 'called');
    writeFileSync(bin, `#!/bin/bash
[[ $# -eq 8 && "$1" == exec && "$2" == --expect && "$3" == personal && "$4" == --no-resolve && "$5" == -- && "$6" == sh && "$7" == -c && "$8" == 'test "$LOCUS_BINDING" = personal && echo "sample exec succeeded"' ]] || exit 91
printf called > "$CALLED"
case "$FAULT" in
  exit) exit 1 ;;
  output) echo 'unexpected output'; exit 0 ;;
  warning) echo 'warning: synthetic unset-variable warning' >&2 ;;
esac
LOCUS_BINDING=personal exec "$6" "$7" "$8"
`, { mode: 0o755 });
    const result = spawnSync('bash', ['-c', `set -euo pipefail
ok() { printf 'ok  %s\\n' "$*"; }
die() { printf 'FAIL %s\\n' "$*" >&2; exit 1; }
sample_exec_stderr="$1/stderr"
LOCUS_BIN="$2"
${block}
`, 'sample', dir, bin], {
      timeout: 10_000, maxBuffer: 1024 * 1024, cwd: dir, env: { PATH: process.env.PATH, HOME: dir, LOCUS_HOME: dir, CALLED: called, FAULT: fault }, encoding: 'utf8',
    });
    assert.equal(result.signal, null);
    assert.equal(readFileSync(called, 'utf8'), 'called', 'the actual walkthrough argv must reach the stub');
    if (expected) {
      assert.equal(result.status, 1);
      assert.ok(result.stderr.includes(`FAIL ${expected}`), result.stderr);
      assert.doesNotMatch(result.stdout, /ok  sample walkthrough exec succeeds without unset-variable warnings/);
    } else {
      assert.equal(result.status, 0, result.stderr);
      assert.match(result.stdout, /ok  sample walkthrough exec succeeds without unset-variable warnings/);
    }
  });
}
