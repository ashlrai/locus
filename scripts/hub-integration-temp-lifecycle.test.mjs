// Run: node --test scripts/hub-integration-temp-lifecycle.test.mjs
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const source = readFileSync(join(root, 'scripts/hub-integration-test.sh'), 'utf8');
const doc = readFileSync(join(root, 'docs/hub-integration-temp-lifecycle.md'), 'utf8');
const isolation = readFileSync(join(root, 'docs/hub-integration-isolation.md'), 'utf8');

test('guide inventories all integration smoke mktemp directories in creation order', () => {
  const code = source.split('\n').filter((line) => !/^\s*#/.test(line)).join('\n');
  const allocations = [...code.matchAll(/(\w+)="\$\(mktemp -d "\$\{TMPDIR:-\/tmp\}\/([^"/]+)"\)"/g)]
    .map(([, variable, template]) => [variable, template]);
  assert.equal(allocations.length, [...code.matchAll(/\bmktemp\b/g)].length,
    'recognize every mktemp call before comparing the inventory');
  assert.deepEqual(allocations, [['SMOKE_HOME', 'locus-hub-int.XXXXXX'],
    ['SMOKE_PROJ', 'locus-hub-int-proj.XXXXXX']]);
  const documented = [...doc.matchAll(/^\| `(SMOKE_\w+)` \(`([^`]+)`\) \|/gm)]
    .map(([, variable, template]) => [variable, template]);
  assert.deepEqual(documented, allocations);
});

test('guide describes cleanup and separate temporary resources present in the script', () => {
  const trap = source.indexOf('trap cleanup EXIT');
  assert.ok(trap >= 0 && trap < source.indexOf('SMOKE_HOME="$(mktemp -d'));
  for (const marker of ['rm -rf -- "$SMOKE_HOME" "$SMOKE_PROJ"',
    'kill "$MT_SRV_PID"', 'wait "$MT_SRV_PID"',
    '.locus-hub-int-scan.$$.${RANDOM}', 'rm -rf "$SMOKE_PROJ/canary scan"']) {
    assert.ok(source.includes(marker), `script lifecycle marker missing: ${marker}`);
  }
  for (const marker of ['`EXIT` trap', 'second allocation fails', 'signals',
    '.locus-hub-int-scan.$$.${RANDOM}', 'uncatchable kill', 'authority broker']) {
    assert.ok(doc.includes(marker), `guide lifecycle detail missing: ${marker}`);
  }
});

test('isolation walkthrough traces both directories and their uses to the script', () => {
  for (const [variable, template] of [['SMOKE_HOME', 'locus-hub-int.XXXXXX'],
    ['SMOKE_PROJ', 'locus-hub-int-proj.XXXXXX']]) {
    assert.ok(source.includes(`${variable}="$(mktemp -d "\${TMPDIR:-/tmp}/${template}")"`));
    assert.ok(isolation.includes(`\`${variable}\` (\`${template}\`)`));
  }
  for (const marker of ['LOCUS_HOME', 'LOCUS_SESSION_ID', '.mcp.json', 'hub-merged.mcp.json',
    'mt-server.log', 'mt-body.json', 'mt-init.headers', 'EXIT', 'TMPDIR']) {
    assert.ok(source.includes(marker), `script no longer references ${marker}`);
    assert.ok(isolation.includes(marker), `isolation walkthrough is missing ${marker}`);
  }
  assert.match(isolation, /\[`cleanup`\]\(\.\.\/scripts\/hub-integration-test\.sh#L\d+-L\d+\)/);
  assert.match(isolation, /uncatchable kill/);
  assert.match(isolation, /does not redirect the caller's `HOME`/);
});
