// Preserve the platform decision record's evidence limits in its current owner.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');

test('platform decisions remain open and mocked selection is not native evidence', () => {
  const matrix = readFileSync(join(root, 'docs/installer-platform-matrix.md'), 'utf8');
  assert.ok(matrix.includes('https://github.com/ashlrai/locus/issues/169'));
  assert.match(matrix, /open questions or unverified outcomes, not promises of support/);
  assert.match(matrix, /Do not mark a platform accepted solely from the offline tests/);
  assert.match(matrix, /native compatibility and release availability must be\s+checked separately/);
});
