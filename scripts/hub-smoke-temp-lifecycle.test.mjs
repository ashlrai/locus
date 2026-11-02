// Run: node --test scripts/hub-smoke-temp-lifecycle.test.mjs
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-smoke.sh');

test('temporary lifecycle guide tracks allocation order and exit cleanup', () => {
  const source = readFileSync(script, 'utf8');
  const doc = readFileSync(join(dirname(script), '../docs/hub-smoke-temp-lifecycle.md'), 'utf8');
  const allocations = [...source.matchAll(/if ! (SMOKE_\w+)="\$\(mktemp( -d)? "\$\{TMPDIR:-\/tmp\}\/([^"]+)"\)"/g)];
  const rows = doc.split('\n').filter((line) => /^\| [123] \|/.test(line));
  assert.equal(allocations.length, 3, 'review the lifecycle when allocations change');
  assert.equal(rows.length, allocations.length);
  for (const [index, [, variable, flag, template]] of allocations.entries()) {
    assert.ok(rows[index].startsWith(`| ${index + 1} | \`${variable}\``));
    assert.ok(rows[index].includes(`\`${template}\``));
    assert.ok(rows[index].includes(flag ? 'directory' : 'file'));
    assert.ok(rows[index].includes('`EXIT` trap'));
  }
  const trap = source.indexOf('trap cleanup EXIT');
  assert.ok(trap >= 0 && trap < allocations[0].index);
  assert.ok(source.includes('cleanup() { rm -rf -- "$SMOKE_HOME" "$SMOKE_PROJ" "$SMOKE_ERR"; }'));
  for (const marker of [': >"$SMOKE_ERR"', '2>"$SMOKE_ERR"', 'export LOCUS_HOME="$SMOKE_HOME"',
    'cd "$SMOKE_PROJ"', '> .mcp.json']) {
    assert.ok(source.includes(marker), `script lifecycle changed: ${marker}`);
  }
  for (const detail of ['`TMPDIR`', '`/tmp`', 'failed allocation', 'uncatchable termination',
    '`SIGKILL`', '`target/`', 'asynchronous']) {
    assert.ok(doc.includes(detail), `missing lifecycle detail: ${detail}`);
  }
});

