// Run offline: node --test scripts/e2e-syntax.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

test('e2e.sh has valid Bash syntax without executing its commands', (t) => {
  const home = mkdtempSync(join(tmpdir(), 'locus-e2e-syntax-'));
  t.after(() => rmSync(home, { recursive: true, force: true }));
  const script = fileURLToPath(new URL('./e2e.sh', import.meta.url));
  // Parse only, with no ambient BASH_ENV startup hooks or network commands.
  const result = spawnSync('/bin/bash', ['-n', script], {
    env: { HOME: home, PATH: '/usr/bin:/bin' },
    encoding: 'utf8',
    timeout: 5_000,
  });

  assert.ifError(result.error);
  assert.equal(result.signal, null);
  assert.equal(result.status, 0, result.stderr);
});
