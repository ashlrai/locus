// Run: node --test scripts/e2e-map.test.mjs
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');

test('the E2E map covers each logged section in execution order', () => {
  const script = readFileSync(join(root, 'scripts/e2e.sh'), 'utf8');
  const map = readFileSync(join(root, 'docs/e2e-test-map.md'), 'utf8');
  const sections = [...script.matchAll(/^log "(\d+[a-z]?)\./gm)].map((match) => match[1]);
  const rows = [...map.matchAll(/^\| (\d+[a-z]?) \| ([^|\n]+) \| ([^|\n]+) \|$/gm)];

  assert.ok(sections.length > 0, 'no E2E section logs found');
  assert.equal(new Set(sections).size, sections.length, 'duplicate E2E section log');
  assert.deepEqual(rows.map((row) => row[1]), sections);
  for (const [, section, invocation, purpose] of rows) {
    assert.ok(invocation.trim() && purpose.trim(), `section ${section} needs an invocation and purpose`);
  }
});
