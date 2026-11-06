// Restore the installer prerequisite cross-check against its current document owner.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');

test('platform matrix documents every checked installer command and checksum alternative', () => {
  const installer = readFileSync(join(root, 'scripts/install.sh'), 'utf8');
  const matrix = readFileSync(join(root, 'docs/installer-platform-matrix.md'), 'utf8');
  const commands = installer.match(/for tool in ([^;]+); do/);
  assert.ok(commands, 'installer prerequisite loop must be present');
  for (const command of [...commands[1].trim().split(/\s+/), 'sha256sum', 'shasum']) {
    assert.ok(matrix.includes(`\`${command}\``), `matrix missing ${command}`);
  }
});
