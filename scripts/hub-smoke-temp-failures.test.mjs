// Run: node --test scripts/hub-smoke-temp-failures.test.mjs
import assert from 'node:assert/strict';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const script = join(root, 'scripts/hub-smoke.sh');
const doc = readFileSync(join(root, 'docs/hub-smoke-temp-failures.md'), 'utf8');
const source = readFileSync(script, 'utf8');
const allocations = [...source.matchAll(/if ! (SMOKE_\w+)="\$\(mktemp( -d)? "\$\{TMPDIR:-\/tmp\}\/([^"]+)"\)"; then\n  echo "([^"]+)" >&2\n  exit 1\nfi/g)];

test('failure guide tracks allocation order, diagnostics and cleanup', () => {
  const rows = doc.split('\n').filter((line) => /^\| `SMOKE_/.test(line));
  assert.equal(allocations.length, 3, 'review the guide when allocation handling changes');
  assert.equal(rows.length, allocations.length);
  const trap = source.indexOf('trap cleanup EXIT');
  assert.ok(trap >= 0 && trap < allocations[0].index);
  assert.ok(source.includes('cleanup() { rm -rf -- "$SMOKE_HOME" "$SMOKE_PROJ" "$SMOKE_ERR"; }'));
  for (const [index, [, variable, flag, template, diagnostic]] of allocations.entries()) {
    assert.ok(rows[index].startsWith(`| \`${variable}\``));
    for (const detail of [flag ? '`mktemp -d`' : '`mktemp`', `\`${template}\``, `\`${diagnostic}\``]) {
      assert.ok(rows[index].includes(detail), `missing ${detail}`);
    }
    for (const [prior, [, previous]] of allocations.entries()) {
      if (prior < index) assert.ok(rows[index].includes(`\`${previous}\``));
    }
    assert.ok(rows[index].endsWith('| None |'));
  }
});

for (const [index, [, variable, , template, diagnostic]] of allocations.entries()) {
  test(`${variable} allocation failure stops later allocations and cleans earlier paths`, (t) => {
    const fixture = mkdtempSync(join(tmpdir(), 'hub-smoke-failures-'));
    t.after(() => rmSync(fixture, { recursive: true, force: true }));
    const bin = join(fixture, 'bin');
    const scratch = join(fixture, 'scratch with spaces');
    const record = join(fixture, 'attempts');
    mkdirSync(bin);
    mkdirSync(scratch);
    writeFileSync(join(scratch, 'keep'), 'sentinel');
    // Resolve the real utility before intercepting it, without exposing environment values.
    const lookup = spawnSync('which', ['mktemp'], { encoding: 'utf8' });
    assert.equal(lookup.status, 0);
    writeFileSync(join(bin, 'mktemp'), `#!/usr/bin/env bash\nprintf '%s\\n' "\${!#}" >> "$ATTEMPTS"\nif [[ "\${!#}" == */"$FAIL_TEMPLATE" ]]; then exit 1; fi\nexec "$REAL_MKTEMP" "$@"\n`, { mode: 0o755 });
    const result = spawnSync('bash', [script], {
      env: { ...process.env, BASH_ENV: '', TMPDIR: scratch, PATH: `${bin}:${process.env.PATH}`,
        ATTEMPTS: record, FAIL_TEMPLATE: template, REAL_MKTEMP: lookup.stdout.trim() },
      encoding: 'utf8',
    });
    assert.equal(result.status, 1);
    assert.equal(result.signal, null);
    assert.equal(result.stdout, '');
    assert.equal(result.stderr.trim(), diagnostic);
    const attempts = readFileSync(record, 'utf8').trim().split('\n');
    assert.deepEqual(attempts.map((path) => path.slice(scratch.length + 1)),
      allocations.slice(0, index + 1).map((allocation) => allocation[3]));
    assert.deepEqual(readdirSync(scratch), ['keep']);
    assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'sentinel');
    assert.ok(existsSync(scratch));
  });
}
