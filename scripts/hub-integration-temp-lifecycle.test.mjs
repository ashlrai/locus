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

// Resolves every top-level temporary-directory allocation in creation order,
// whether the script allocates directly with mktemp or through a mktemp helper.
// A make_smoke_dir definition wrapping `mktemp -d` under the same parent is
// itself recognized as the mktemp call, and each VAR="$(make_smoke_dir PREFIX)"
// call site allocates a PREFIX.XXXXXX directory.
function smokeAllocations(code) {
  const direct = [...code.matchAll(/(\w+)="\$\(mktemp -d "\$\{TMPDIR:-\/tmp\}\/([^"/]+)"\)"/g)];
  const helper = /make_smoke_dir\(\) \{\n\s*mktemp -d "\$\{TMPDIR:-\/tmp\}\/\$1\.XXXXXX"\n\}/.test(code);
  assert.equal(direct.length + (helper ? 1 : 0), [...code.matchAll(/\bmktemp\b/g)].length,
    'recognize every mktemp call before comparing the inventory');
  return [...code.matchAll(/(\w+)="\$\((?:mktemp -d "\$\{TMPDIR:-\/tmp\}\/([^"/]+)"|make_smoke_dir ([^")]+))\)"/g)]
    .map(([, variable, template, prefix]) => [variable, template ?? `${prefix}.XXXXXX`]);
}

test('guide inventories all integration smoke mktemp directories in creation order', () => {
  const code = source.split('\n').filter((line) => !/^\s*#/.test(line)).join('\n');
  const allocations = smokeAllocations(code);
  assert.deepEqual(allocations, [['SMOKE_HOME', 'locus-hub-int.XXXXXX'],
    ['SMOKE_PROJ', 'locus-hub-int-proj.XXXXXX']]);
  const documented = [...doc.matchAll(/^\| `(SMOKE_\w+)` \(`([^`]+)`\) \|/gm)]
    .map(([, variable, template]) => [variable, template]);
  assert.deepEqual(documented, allocations);
});

test('guide describes cleanup and separate temporary resources present in the script', () => {
  const trap = source.indexOf('trap cleanup EXIT');
  const firstAllocation = Math.min(
    ...['SMOKE_HOME="$(mktemp -d', 'SMOKE_HOME="$(make_smoke_dir ']
      .map((marker) => {
        const index = source.indexOf(marker);
        return index === -1 ? Infinity : index;
      }));
  assert.ok(trap >= 0 && trap < firstAllocation);
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
    const direct = `${variable}="$(mktemp -d "\${TMPDIR:-/tmp}/${template}")"`;
    const helperCall = `${variable}="$(make_smoke_dir ${template.slice(0, -'.XXXXXX'.length)})"`;
    assert.ok(source.includes(direct) || source.includes(helperCall),
      `${variable} must allocate ${template} beneath \${TMPDIR:-/tmp}`);
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
