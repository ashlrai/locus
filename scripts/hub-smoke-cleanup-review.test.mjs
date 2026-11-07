// Run: node --test scripts/hub-smoke-cleanup-review.test.mjs
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const script = join(root, 'scripts/hub-smoke.sh');
const source = readFileSync(script, 'utf8');
const doc = readFileSync(join(root, 'docs/hub-smoke-cleanup-review.md'), 'utf8');

test('review identifies each temporary allocation and its cleanup path', () => {
  const allocations = [...source.matchAll(/if ! (SMOKE_\w+)="\$\(mktemp( -d)? "\$\{TMPDIR:-\/tmp\}\/([^"]+)"\)"/g)];
  assert.equal(allocations.length, 3, 'review the doc when allocations change');
  for (const [, variable, flag, template] of allocations) {
    const row = doc.split('\n').find((line) => line.startsWith(`| \`${variable}\``));
    assert.ok(row, `missing resource row: ${variable}`);
    assert.ok(row.includes(`\`${template}\``), `missing template: ${template}`);
    assert.ok(row.includes(flag ? 'directory' : 'file'), `wrong kind for ${variable}`);
    assert.ok(row.includes('`EXIT` trap'), `${variable} must name its cleanup path`);
  }
  assert.ok(source.includes('cleanup() { rm -rf -- "$SMOKE_HOME" "$SMOKE_PROJ" "$SMOKE_ERR"; }'),
    'script cleanup function changed');
  const trap = source.indexOf('trap cleanup EXIT');
  assert.ok(trap >= 0 && trap < allocations[0].index,
    'the trap must be installed before the first allocation');
});

test('review covers dependent resources and their non-trap cleanup paths', () => {
  for (const [marker, detail] of [
    ['> .mcp.json', '`.mcp.json`'],
    ['locus leave', '`locus leave`'],
    ['locus mcp revoke', '`locus mcp revoke`'],
    ['locus mcp mint --binding personal', '`locus mcp mint`'],
    ['LOCUS_CONTROL_CAPABILITY', '`LOCUS_CONTROL_CAPABILITY`'],
    ['MINT_JSON', '`MINT_JSON`'],
    ['cargo build -q -p locus-cli', '`target/`'],
  ]) {
    assert.ok(source.includes(marker), `script changed: ${marker}`);
    assert.ok(doc.includes(detail), `missing resource coverage: ${detail}`);
  }
  for (const detail of ['locus-authority', 'asynchronous', 'delegated session',
    'never written to disk']) {
    assert.ok(doc.includes(detail), `missing cleanup-path detail: ${detail}`);
  }
});

test('review enumerates each early exit the script supports', () => {
  const explicit = [...source.matchAll(/echo "(error: [^"]+)" >&2\n  exit 1/g)].map((m) => m[1]);
  assert.equal(explicit.length, 4, 'review the doc when explicit exits change');
  for (const diagnostic of explicit) {
    assert.ok(doc.includes(`\`${diagnostic}\``), `missing early exit: ${diagnostic}`);
  }
  for (const detail of ['`set -e`', 'init/pin failed after', 'cargo build',
    '== hub-smoke FAILED ==', '== hub-smoke OK ==', '`SIGTERM`', '`SIGKILL`',
    'uncatchable kill', 'between `mktemp`', 'unrecorded', '`rm -rf`',
    'banner']) {
    assert.ok(doc.includes(detail), `missing exit or gap coverage: ${detail}`);
  }
});
