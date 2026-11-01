// Run: node --test scripts/hub-integration-tmpdir.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { basename, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const script = join(root, 'scripts/hub-integration-test.sh');
const doc = readFileSync(join(root, 'docs/hub-integration-tmpdir.md'), 'utf8');

// Stop at init after recording the real mktemp directories; the EXIT trap
// still runs. Capture script output because it includes the temporary home.
const harness = `
node() { :; }
jq() { :; }
cargo() { :; }
mktemp() {
  local path
  path="$(command mktemp "$@")" || return
  printf '%s\\n' "$path" >> "$RECORD"
  printf '%s\\n' "$path"
}
locus() {
  if [[ "$1" == init ]]; then
    return 73
  fi
}
export -f node jq cargo locus mktemp
exec bash "$1"
`;

for (const setting of ['unset', 'empty']) {
  test(`${setting} TMPDIR falls back to /tmp for both directories`, (t) => {
    const scratch = mkdtempSync('/tmp/locus-tmpdir-test-');
    t.after(() => rmSync(scratch, { recursive: true, force: true }));
    const record = join(scratch, 'home');
    const env = { PATH: process.env.PATH, HOME: process.env.HOME, RECORD: record };
    if (setting === 'empty') env.TMPDIR = '';
    const result = spawnSync('bash', ['-c', harness, 'tmpdir-test', script], {
      env, encoding: 'utf8',
    });
    assert.equal(result.error, undefined);
    assert.equal(result.status, 73);
    const paths = readFileSync(record, 'utf8').trim().split('\n');
    assert.equal(paths.length, 2);
    assert.match(basename(paths[0]), /^locus-hub-int\.[^/]+$/);
    assert.match(basename(paths[1]), /^locus-hub-int-proj\.[^/]+$/);
    for (const path of paths) {
      assert.equal(dirname(path), '/tmp');
      assert.equal(existsSync(path), false);
    }
  });
}

test('failure of the second mktemp removes the populated first directory', (t) => {
  const scratch = mkdtempSync('/tmp/locus-second-mktemp-');
  t.after(() => rmSync(scratch, { recursive: true, force: true }));
  const first = join(scratch, 'first');
  const second = join(scratch, 'second-attempt');
  const init = join(scratch, 'init-attempt');
  writeFileSync(join(scratch, 'keep'), 'sentinel');

  const failureHarness = `
node() { :; }
jq() { :; }
cargo() { :; }
locus() { if [[ "$1" == init ]]; then : > "$INIT"; fi; }
mktemp() {
  if [[ -f "$FIRST" ]]; then
    [[ -f "$(cat "$FIRST")/populated" ]] || return 74
    : > "$SECOND"
    return 73
  fi
  local path
  path="$(command mktemp "$@")" || return
  : > "$path/populated"
  printf '%s\\n' "$path" > "$FIRST"
  printf '%s\\n' "$path"
}
export -f node jq cargo locus mktemp
exec bash "$1"
`;
  const result = spawnSync('bash', ['-c', failureHarness, 'second-mktemp-test', script], {
    env: { PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: scratch,
      FIRST: first, SECOND: second, INIT: init },
    encoding: 'utf8',
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 73, 'the second allocation failure must propagate');
  assert.equal(existsSync(second), true, 'the second allocation must see the populated first directory');
  assert.equal(existsSync(init), false, 'initialization must not run after allocation fails');
  const path = readFileSync(first, 'utf8').trim();
  assert.equal(dirname(path), scratch);
  assert.match(basename(path), /^locus-hub-int\.[^/]+$/);
  assert.equal(existsSync(path), false, 'the first directory must be removed by the exit trap');
  assert.deepEqual(readdirSync(scratch).sort(), ['first', 'keep', 'second-attempt']);
  assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'sentinel');
});

test('guide inventories the script templates and fallback', () => {
  const source = readFileSync(script, 'utf8');
  for (const [variable, template] of [
    ['SMOKE_HOME', 'locus-hub-int.XXXXXX'],
    ['SMOKE_PROJ', 'locus-hub-int-proj.XXXXXX'],
  ]) {
    assert.ok(source.includes(`${variable}="$(mktemp -d "\${TMPDIR:-/tmp}/${template}")"`));
    assert.ok(doc.includes(`\`${variable}\` | \`${template}\``));
  }
  assert.ok(source.includes('listing="${TMPDIR:-/tmp}/.locus-hub-int-scan.$$.${RANDOM}"'));
  assert.ok(doc.includes('`.locus-hub-int-scan.$$.${RANDOM}`'));
  assert.match(doc, /unset \*\*or empty\*\*, Bash selects `\/tmp`/);
});
