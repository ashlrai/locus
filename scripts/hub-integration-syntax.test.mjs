// Run offline: node --test scripts/hub-integration-syntax.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

test('hub-integration-test.sh has valid Bash syntax without executing integration commands', (t) => {
  const home = mkdtempSync(join(tmpdir(), 'locus-hub-integration-syntax-'));
  t.after(() => rmSync(home, { recursive: true, force: true }));
  const script = fileURLToPath(new URL('./hub-integration-test.sh', import.meta.url));
  // Parse only: no Cargo build, Locus pin, credentials or network access.
  // An explicit environment also excludes ambient BASH_ENV startup hooks.
  const result = spawnSync('/bin/bash', ['-n', script], {
    env: { HOME: home, PATH: '/usr/bin:/bin' },
    encoding: 'utf8',
    timeout: 5_000,
  });

  assert.ifError(result.error);
  assert.equal(result.signal, null);
  assert.equal(result.status, 0, result.stderr);
});
