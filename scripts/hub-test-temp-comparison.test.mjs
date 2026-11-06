// Run: node --test scripts/hub-test-temp-comparison.test.mjs
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const doc = readFileSync(join(root, 'docs/hub-test-temp-comparison.md'), 'utf8');

test('comparison inventories every mktemp allocation in both hub scripts', () => {
  const allocations = [];
  for (const script of ['hub-smoke.sh', 'hub-integration-test.sh']) {
    const source = readFileSync(join(root, 'scripts', script), 'utf8');
    const matches = [...source.matchAll(/(\w+)="\$\(mktemp( -d)? "\$\{TMPDIR:-\/tmp\}\/([^"/]+)"\)"/g)];
    const code = source.split('\n').filter((line) => !/^\s*#/.test(line)).join('\n');
    for (const [, variable, directory, template] of matches) {
      allocations.push([script, variable, directory ? 'directory' : 'file', template]);
    }
    let recognized = matches.length;
    // A make_smoke_dir helper wraps `mktemp -d` beneath the same parent; the
    // definition accounts for one mktemp call and every VAR="$(make_smoke_dir
    // PREFIX)" call site allocates a PREFIX.XXXXXX directory.
    if (/make_smoke_dir\(\) \{\n\s*mktemp -d "\$\{TMPDIR:-\/tmp\}\/\$1\.XXXXXX"\n\}/.test(code)) {
      recognized += 1;
      const calls = [...code.matchAll(/(\w+)="\$\(make_smoke_dir ([^")]+)\)"/g)];
      assert.equal(calls.length, [...code.matchAll(/\$\(make_smoke_dir /g)].length,
        `${script}: every make_smoke_dir call must be recognized`);
      for (const [, variable, prefix] of calls) {
        allocations.push([script, variable, 'directory', `${prefix}.XXXXXX`]);
      }
    }
    assert.equal(recognized, [...code.matchAll(/\bmktemp\b/g)].length,
      `${script}: every mktemp allocation must be recognized`);
    assert.ok(doc.includes(`../scripts/${script}`), `${script}: source link missing`);
  }
  const documented = [...doc.matchAll(/^\| `(hub-(?:smoke|integration-test)\.sh)` \| `(\w+)` \| (directory|file) \| `([^`]+)` \| [^|\n]+ \|$/gm)]
    .map(([, script, variable, type, template]) => [script, variable, type, template]);
  assert.deepEqual(documented.sort(), allocations.sort());
});

test('comparison covers integration-only fixtures and scan listings', () => {
  const source = readFileSync(join(root, 'scripts/hub-integration-test.sh'), 'utf8');
  for (const path of ['hub-merged.mcp.json', 'mt-server.log', 'mt-body.json', 'mt-init.headers',
    'canary scan/nested dir', 'clean file', 'planted file', '.locus-hub-int-scan.']) {
    assert.ok(source.includes(path), `script no longer references ${path}`);
    assert.ok(doc.includes(path), `comparison is missing ${path}`);
  }
});
