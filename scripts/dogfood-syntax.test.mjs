// Run offline: node --test scripts/dogfood-syntax.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

test('dogfood.sh has valid Bash syntax without executing commands', (t) => {
  const home = mkdtempSync(join(tmpdir(), 'locus-dogfood-syntax-'));
  t.after(() => rmSync(home, { recursive: true, force: true }));
  const script = fileURLToPath(new URL('./dogfood.sh', import.meta.url));
  // Parse only, with no ambient BASH_ENV hooks or network access.
  const check = (path) => spawnSync('/bin/bash', ['-n', path], {
    env: { HOME: home, PATH: '/usr/bin:/bin' },
    encoding: 'utf8',
    timeout: 5_000,
  });

  const result = check(script);
  assert.ifError(result.error);
  assert.equal(result.signal, null);
  assert.equal(result.status, 0, result.stderr);

  const invalid = join(home, 'invalid.sh');
  writeFileSync(invalid, 'if true; then\n');
  const rejection = check(invalid);
  assert.ifError(rejection.error);
  assert.equal(rejection.signal, null);
  assert.notEqual(rejection.status, 0, 'invalid syntax must fail');
});
