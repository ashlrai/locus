// Run: node --test scripts/hub-smoke-mktemp-partial.test.mjs
// A mktemp that creates its resource and then exits nonzero (e.g. chmod or
// flush failure after creation) must still fail hub-smoke and leave nothing.
import assert from 'node:assert/strict';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const script = join(root, 'scripts/hub-smoke.sh');
const source = readFileSync(script, 'utf8');
const allocations = [...source.matchAll(/if ! (SMOKE_\w+)="\$\(mktemp( -d)? "\$\{TMPDIR:-\/tmp\}\/([^"]+)"\)"; then\n  echo "([^"]+)" >&2\n  exit 1\nfi/g)];

test('allocation sites are discoverable', () => assert.equal(allocations.length, 3));

for (const [index, [, variable, , template, diagnostic]] of allocations.entries()) {
  test(`${variable}: mktemp failing after creating its path leaves nothing behind`, (t) => {
    const fixture = mkdtempSync(join(tmpdir(), 'hub-smoke-partial-'));
    t.after(() => rmSync(fixture, { recursive: true, force: true }));
    const bin = join(fixture, 'bin');
    const scratch = join(fixture, 'scratch with spaces');
    mkdirSync(bin);
    mkdirSync(scratch);
    writeFileSync(join(scratch, 'keep'), 'sentinel');
    const lookup = spawnSync('which', ['mktemp'], { encoding: 'utf8' });
    assert.equal(lookup.status, 0);
    writeFileSync(join(bin, 'mktemp'), `#!/usr/bin/env bash
path="$("$REAL_MKTEMP" "$@")" || exit 1
if [[ "\${!#}" == */"$FAIL_TEMPLATE" ]]; then printf '%s\\n' "$path"; exit 1; fi
printf '%s\\n' "$path"
`, { mode: 0o755 });
    const result = spawnSync('bash', [script], {
      env: { ...process.env, BASH_ENV: '', TMPDIR: scratch, PATH: `${bin}:${process.env.PATH}`,
        FAIL_TEMPLATE: template, REAL_MKTEMP: lookup.stdout.trim() },
      encoding: 'utf8',
    });
    assert.equal(result.status, 1);
    assert.equal(result.signal, null);
    assert.equal(result.stdout, '');
    assert.equal(result.stderr.trim(), diagnostic);
    assert.deepEqual(readdirSync(scratch), ['keep'], `allocation ${index} must not leak`);
    assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'sentinel');
    assert.ok(existsSync(scratch));
  });
}
