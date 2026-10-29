// Run: node --test scripts/hub-smoke.test.mjs
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-smoke.sh');
const realMktemp = execFileSync('which', ['mktemp'], { encoding: 'utf8' }).trim();

test('invalid TMPDIR fails clearly before starting the smoke test', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'hub-smoke-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const result = spawnSync('bash', [script], {
    env: { ...process.env, TMPDIR: join(root, 'missing', 'nested') },
    encoding: 'utf8',
  });

  assert.notEqual(result.status, 0);
  assert.equal(result.signal, null);
  assert.match(result.stderr, /error: hub-smoke could not create temporary home/);
  assert.equal(result.stdout, '');
  assert.deepEqual(readdirSync(root), []);
});

for (const [template, message] of [
  ['locus-hub-smoke.XXXXXX', 'temporary home'],
  ['locus-hub-proj.XXXXXX', 'temporary project directory'],
  ['locus-hub-smoke-err.XXXXXX', 'temporary error file'],
]) {
  test(`failure creating ${message} stops and cleans up`, (t) => {
    const root = mkdtempSync(join(tmpdir(), 'hub-smoke-test-'));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const bin = join(root, 'bin');
    const scratch = join(root, 'scratch');
    mkdirSync(bin);
    mkdirSync(scratch);
    writeFileSync(join(bin, 'mktemp'), `#!/usr/bin/env bash
for arg in "$@"; do
  if [[ "$arg" == */"$FAIL_TEMPLATE" ]]; then exit 1; fi
done
exec "$REAL_MKTEMP" "$@"
`, { mode: 0o755 });
    const result = spawnSync('bash', [script], {
      env: { ...process.env, TMPDIR: scratch, PATH: `${bin}:${process.env.PATH}`, FAIL_TEMPLATE: template, REAL_MKTEMP: realMktemp },
      encoding: 'utf8',
    });

    assert.notEqual(result.status, 0);
    assert.equal(result.signal, null);
    assert.match(result.stderr, new RegExp(`error: hub-smoke could not create ${message}`));
    assert.equal(result.stdout, '');
    assert.deepEqual(readdirSync(scratch), []);
  });
}
