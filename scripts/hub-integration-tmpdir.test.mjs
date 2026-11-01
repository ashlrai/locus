// Run: node --test scripts/hub-integration-tmpdir.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
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

for (const [failedTemplate, expectedAttempts] of [
  ['locus-hub-int.XXXXXX', ['locus-hub-int.XXXXXX']],
  ['locus-hub-int-proj.XXXXXX', ['locus-hub-int.XXXXXX', 'locus-hub-int-proj.XXXXXX']],
]) {
  test(`failure creating ${failedTemplate} stops execution and cleans earlier directories`, (t) => {
    const scratch = mkdtempSync('/tmp/locus-tmp-failure-');
    t.after(() => rmSync(scratch, { recursive: true, force: true }));
    const temp = join(scratch, 'temp [with spaces]');
    const attempts = join(scratch, 'attempts');
    const created = join(scratch, 'created');
    const reached = join(scratch, 'init-reached');
    mkdirSync(temp);
    writeFileSync(join(temp, 'keep'), 'sentinel');

    const failAllocation = `
node() { :; }
jq() { :; }
cargo() { :; }
locus() { [[ "$1" != init ]] || : > "$REACHED"; }
mktemp() {
  local template="${'$'}{!#}" path
  printf '%s\\n' "${'$'}{template##*/}" >> "$ATTEMPTS"
  [[ "${'$'}{template##*/}" != "$FAIL_TEMPLATE" ]] || return 73
  path="$(command mktemp "$@")" || return
  touch "$path/fixture" || return
  printf '%s\\n' "$path" >> "$CREATED"
  printf '%s\\n' "$path"
}
export -f node jq cargo locus mktemp
exec bash "$1"
`;
    const result = spawnSync('bash', ['-c', failAllocation, 'failure-test', script], {
      env: { PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: temp,
        ATTEMPTS: attempts, CREATED: created, REACHED: reached, FAIL_TEMPLATE: failedTemplate },
      encoding: 'utf8',
    });
    assert.equal(result.error, undefined);
    assert.equal(result.signal, null);
    assert.equal(result.status, 73, 'the allocation failure must propagate');
    assert.deepEqual(readFileSync(attempts, 'utf8').trim().split('\n'), expectedAttempts);
    const paths = existsSync(created) ? readFileSync(created, 'utf8').trim().split('\n') : [];
    assert.equal(paths.length, expectedAttempts.length - 1);
    for (const path of paths) {
      assert.equal(dirname(path), temp);
      assert.equal(existsSync(path), false, 'the populated earlier directory must be removed');
    }
    assert.equal(existsSync(reached), false, 'initialization must not run after allocation failure');
    assert.deepEqual(readdirSync(temp), ['keep']);
    assert.equal(readFileSync(join(temp, 'keep'), 'utf8'), 'sentinel');
  });
}

test('custom TMPDIR creates and cleans both directories without touching a glob-matching neighbor', (t) => {
  const scratch = mkdtempSync('/tmp/locus-tmpdir-test-');
  t.after(() => rmSync(scratch, { recursive: true, force: true }));
  const custom = join(scratch, 'tmp[ab]');
  const neighbor = join(scratch, 'tmpa');
  const record = join(scratch, 'observed');
  mkdirSync(custom);
  mkdirSync(neighbor);
  writeFileSync(join(custom, 'keep'), 'custom sentinel');

  // Inspect the actual directories before forcing an early exit and its cleanup trap.
  const observe = `
node() { :; }
jq() { :; }
cargo() { :; }
locus() {
  if [[ "$1" == init ]]; then
    local projects=("$TMPDIR"/locus-hub-int-proj.*) project
    [[ -d "$LOCUS_HOME" && "${'$'}{#projects[@]}" == 1 && -d "${'$'}{projects[0]}" ]] || return 74
    project="${'$'}{projects[0]}"
    [[ "$LOCUS_HOME" != "$project" ]] || return 75
    mkdir "$NEIGHBOR/${'$'}{LOCUS_HOME##*/}" "$NEIGHBOR/${'$'}{project##*/}" || return 76
    touch "$LOCUS_HOME/fixture" "$project/fixture" || return 77
    printf '%s\\n' "$LOCUS_HOME" "$project" > "$RECORD"
    return 73
  fi
}
export -f node jq cargo locus
exec bash "$1"
`;
  const result = spawnSync('bash', ['-c', observe, 'tmpdir-test', script], {
    env: { PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: custom, NEIGHBOR: neighbor, RECORD: record },
    encoding: 'utf8',
  });
  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 73, 'the injected init failure must trigger cleanup');
  const paths = readFileSync(record, 'utf8').trim().split('\n');
  assert.equal(paths.length, 2);
  assert.match(basename(paths[0]), /^locus-hub-int\.[^/]+$/);
  assert.match(basename(paths[1]), /^locus-hub-int-proj\.[^/]+$/);
  for (const path of paths) {
    assert.equal(dirname(path), custom, 'each real directory must be directly under TMPDIR');
    assert.equal(existsSync(path), false, 'each populated directory must be removed');
    assert.equal(existsSync(join(neighbor, basename(path))), true, 'cleanup must preserve matching neighbors');
  }
  assert.deepEqual(readdirSync(custom), ['keep']);
  assert.equal(readFileSync(join(custom, 'keep'), 'utf8'), 'custom sentinel');
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
