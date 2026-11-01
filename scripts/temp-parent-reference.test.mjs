// Run: node --test scripts/temp-parent-reference.test.mjs
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const doc = readFileSync(join(root, 'docs/temp-parent-reference.md'), 'utf8');

test('reference matches both scripts’ complete mktemp inventories', () => {
  const allocations = [];
  for (const script of ['hub-smoke.sh', 'hub-integration-test.sh']) {
    const source = readFileSync(join(root, 'scripts', script), 'utf8');
    const code = source.split('\n').filter((line) => !/^\s*#/.test(line)).join('\n');
    const matches = [...code.matchAll(/(\w+)="\$\(mktemp( -d)? "\$\{TMPDIR:-\/tmp\}\/([^"/]+)"\)"/g)];
    assert.equal(matches.length, [...code.matchAll(/\bmktemp\b/g)].length,
      `${script}: recognize every mktemp call`);
    for (const [, variable, directory, template] of matches) {
      allocations.push([script, variable, directory ? 'directory' : 'file', template]);
    }
    assert.ok(doc.includes(`../scripts/${script}`));
  }
  const rows = [...doc.matchAll(/^\| `(hub-(?:smoke|integration-test)\.sh)` \| `(\w+)` \| (directory|file) \| `([^`]+)` \|$/gm)]
    .map(([, script, variable, type, template]) => [script, variable, type, template]);
  assert.deepEqual(rows.sort(), allocations.sort());
  assert.ok(doc.includes('`${TMPDIR:-/tmp}`'));
  assert.match(doc, /unset or empty/);
});

test('reference identifies the separately constructed scan listing', () => {
  const source = readFileSync(join(root, 'scripts/hub-integration-test.sh'), 'utf8');
  const listing = '${TMPDIR:-/tmp}/.locus-hub-int-scan.$$.${RANDOM}';
  assert.ok(source.includes(`listing="${listing}"`));
  assert.ok(doc.includes(`\`${listing}\``));
  assert.ok(source.includes('rm -f -- "${TMPDIR:-/tmp}"/.locus-hub-int-scan.$$.*'));
});
