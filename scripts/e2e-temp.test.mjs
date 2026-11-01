// Run: node --test scripts/e2e-temp.test.mjs
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const script = join(root, 'scripts', 'e2e.sh');

test('temporary directory reference matches the E2E script', () => {
  const source = readFileSync(script, 'utf8');
  const doc = readFileSync(join(root, 'docs', 'e2e-temp-resource-types.md'), 'utf8');
  const created = [...source.matchAll(/^\s*(\w+)="\$\(mktemp -d "\$\{TMPDIR:-\/tmp\}\/([^/"\s]+)XXXXXX"\)"/gm)]
    .map(([, variable, prefix]) => [variable, prefix]);
  const documented = [...doc.matchAll(/^\| `(\w+)` \| `([^`]+)` \| ([^|\n]+) \|$/gm)]
    .map(([, variable, prefix, purpose]) => {
      assert.ok(purpose.trim(), `${variable} needs a purpose`);
      return [variable, prefix];
    });

  assert.equal(created.length, 5, 'expected five temporary directory assignments');
  assert.equal([...source.matchAll(/\bmktemp\s+-d\b/g)].length, created.length,
    'every mktemp directory must be represented in the inventory');
  assert.equal(new Set(created.map(([variable]) => variable)).size, created.length);
  assert.deepEqual(documented.sort(), created.sort(), 'documented variables and prefixes must match the script');
});

function setup(t) {
  const root = mkdtempSync(join(tmpdir(), 'locus-e2e-temp-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const bin = join(root, 'bin');
  mkdirSync(bin);
  // Avoid building binaries and fail immediately after the cleanup trap is set.
  writeFileSync(join(bin, 'cargo'), '#!/bin/sh\nexit 0\n', { mode: 0o755 });
  const locus = join(bin, 'locus');
  writeFileSync(locus, '#!/bin/sh\nexit 1\n', { mode: 0o755 });
  return { root, bin, locus };
}

function run({ bin, locus }, temp, extraEnv = {}) {
  return spawnSync('bash', [script], {
    env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, TMPDIR: temp,
      LOCUS_BIN: locus, MCP_BIN: locus, ...extraEnv },
    encoding: 'utf8',
  });
}

test('uses TMPDIR and cleans the isolated home after a later failure', (t) => {
  const fixture = setup(t);
  const scratch = join(fixture.root, 'scratch');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  const result = run(fixture, scratch);

  assert.notEqual(result.status, 0);
  assert.equal(result.signal, null);
  assert.match(result.stderr, /fresh quickstart failed/);
  assert.deepEqual(readdirSync(scratch), ['keep']);
});

for (const [label, temp] of [['unset', undefined], ['empty', '']]) {
  test(`${label} TMPDIR falls back to /tmp`, (t) => {
    const fixture = setup(t);
    const scratch = join(fixture.root, 'scratch');
    const capture = join(fixture.root, 'template');
    mkdirSync(scratch);
    const realMktemp = execFileSync('which', ['mktemp'], { encoding: 'utf8' }).trim();
    // Redirect only the main home; security probes must use their own TMPDIR.
    writeFileSync(join(fixture.bin, 'mktemp'), `#!/bin/sh
case "$2" in
  */locus-e2e.XXXXXX)
    printf '%s' "$2" > "$PROBE_CAPTURE"
    exec "$REAL_MKTEMP" -d "$PROBE_SCRATCH/locus-e2e.XXXXXX"
    ;;
esac
exec "$REAL_MKTEMP" "$@"
`, { mode: 0o755 });
    const result = run(fixture, temp, {
      REAL_MKTEMP: realMktemp, PROBE_CAPTURE: capture, PROBE_SCRATCH: scratch,
    });

    assert.notEqual(result.status, 0);
    assert.equal(result.signal, null);
    assert.match(result.stderr, /fresh quickstart failed/);
    assert.equal(readFileSync(capture, 'utf8'), '/tmp/locus-e2e.XXXXXX');
    assert.deepEqual(readdirSync(scratch), []);
  });
}

test('a nonexistent TMPDIR fails before creating an isolated home', (t) => {
  const fixture = setup(t);
  const result = run(fixture, join(fixture.root, 'missing'));

  assert.notEqual(result.status, 0);
  assert.equal(result.signal, null);
  assert.match(result.stderr, /mktemp/);
  assert.deepEqual(readdirSync(fixture.root), ['bin']);
});
