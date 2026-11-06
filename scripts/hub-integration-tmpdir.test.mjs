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

// An unusable TMPDIR — not a directory, or simply absent — must abort the run
// before any integration work: real mktemp fails (or, for a relative value,
// the `cd` normalization does), `set -e` turns that into a nonzero exit, and
// the script never reaches `locus init`.
for (const [label, unusableTmpdir, extraCheck] of [
  ['a regular file', (scratch) => {
    const file = join(scratch, 'tmpdir file');
    writeFileSync(file, 'sentinel');
    return file;
  }, (tmpdir) => assert.equal(readFileSync(tmpdir, 'utf8'), 'sentinel',
    'cleanup must not touch the TMPDIR path itself')],
  ['a missing absolute path', (scratch) => join(scratch, 'missing', 'nested'),
    (tmpdir) => assert.equal(existsSync(tmpdir), false,
      'cleanup must not create the missing parent')],
  ['a missing relative path', () => 'missing relative',
    (tmpdir, scratch) => assert.equal(existsSync(join(scratch, tmpdir)), false,
      'the failed normalization must not leave a directory behind')],
]) {
  test(`unusable TMPDIR (${label}) exits nonzero without running the integration workload`, (t) => {
    const scratch = mkdtempSync('/tmp/locus-tmpdir-unusable-');
    t.after(() => rmSync(scratch, { recursive: true, force: true }));
    const calls = join(scratch, 'locus-calls');
    const home = join(scratch, 'home');
    mkdirSync(home);
    const tmpdir = unusableTmpdir(scratch);

    const recordCalls = `
node() { :; }
jq() { :; }
cargo() { :; }
locus() { printf '%s\\n' "$*" >> "$CALLS"; }
export -f node jq cargo locus
exec bash "$1"
`;
    const result = spawnSync('bash', ['-c', recordCalls, 'unusable-tmpdir', script], {
      cwd: scratch,
      env: { PATH: process.env.PATH, HOME: home, TMPDIR: tmpdir, CALLS: calls },
      encoding: 'utf8',
    });
    assert.equal(result.error, undefined);
    assert.equal(result.signal, null);
    assert.equal(result.status, 1, 'an unusable TMPDIR must abort the run');
    assert.equal(result.stdout, '', 'no banner or check output may print');
    assert.match(result.stderr, /mktemp|cd:/, 'the failing step must report to stderr');
    const observed = existsSync(calls) ? readFileSync(calls, 'utf8').trim().split('\n') : [];
    assert.deepEqual(observed, [], 'no Locus command may run before allocation succeeds');
    assert.deepEqual(readdirSync(scratch).filter((entry) => entry.startsWith('locus-hub-int')), [],
      'no smoke directory may be created');
    extraCheck(tmpdir, scratch);
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
    const direct = `${variable}="$(mktemp -d "\${TMPDIR:-/tmp}/${template}")"`;
    const helperCall = `${variable}="$(make_smoke_dir ${template.slice(0, -'.XXXXXX'.length)})"`;
    assert.ok(source.includes(direct) || source.includes(helperCall),
      `${variable} must allocate ${template} beneath \${TMPDIR:-/tmp}`);
    assert.ok(doc.includes(`\`${variable}\` | \`${template}\``));
  }
  assert.ok(source.includes('listing="${TMPDIR:-/tmp}/.locus-hub-int-scan.$$.${RANDOM}"'));
  assert.ok(doc.includes('`.locus-hub-int-scan.$$.${RANDOM}`'));
  assert.match(doc, /unset \*\*or empty\*\*, Bash selects `\/tmp`/);
});

// Plain whitespace edge cases (no glob/metacharacters): word splitting or
// trimming of TMPDIR would relocate or leak a directory.
for (const name of ['my tmp dir', ' lead and trail ', 'two  spaces']) {
  test(`TMPDIR ${JSON.stringify(name)} holds both created directories`, (t) => {
    const scratch = mkdtempSync('/tmp/locus-tmpdir-space-');
    t.after(() => rmSync(scratch, { recursive: true, force: true }));
    const custom = join(scratch, name);
    const record = join(scratch, 'observed');
    mkdirSync(custom);
    writeFileSync(join(custom, 'keep'), 'sentinel');

    const observe = `
node() { :; }
jq() { :; }
cargo() { :; }
locus() {
  if [[ "$1" == init ]]; then
    local projects=("$TMPDIR"/locus-hub-int-proj.*)
    [[ -d "$LOCUS_HOME" && "${'$'}{LOCUS_HOME%/*}" == "$TMPDIR" ]] || return 74
    [[ "${'$'}{#projects[@]}" == 1 && -d "${'$'}{projects[0]}" ]] || return 75
    printf '%s\\n' "$LOCUS_HOME" "${'$'}{projects[0]}" > "$RECORD"
    return 73
  fi
}
export -f node jq cargo locus
exec bash "$1"
`;
    const result = spawnSync('bash', ['-c', observe, 'space-test', script], {
      env: { PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: custom, RECORD: record },
      encoding: 'utf8',
    });
    assert.equal(result.error, undefined);
    assert.equal(result.signal, null);
    assert.equal(result.status, 73, 'the injected init failure must propagate');
    const paths = readFileSync(record, 'utf8').trim().split('\n');
    assert.equal(paths.length, 2);
    assert.match(basename(paths[0]), /^locus-hub-int\.[^/]+$/);
    assert.match(basename(paths[1]), /^locus-hub-int-proj\.[^/]+$/);
    for (const path of paths) {
      assert.equal(dirname(path), custom, 'each directory must be directly under the spaced TMPDIR');
      assert.equal(existsSync(path), false, 'each directory must be removed on exit');
    }
    assert.deepEqual(readdirSync(custom), ['keep']);
    assert.deepEqual(readdirSync(scratch).sort(), [name, 'observed'].sort());
  });
}

// Creation alone is not enough: the script must also work inside SMOKE_PROJ.
// Stop at the first locus call after the cd and check the working directory
// and the .mcp.json written there are beneath the spaced TMPDIR.
test('SMOKE_PROJ under a spaced TMPDIR is entered and receives .mcp.json', (t) => {
  const scratch = mkdtempSync('/tmp/locus-tmpdir-proj-');
  t.after(() => rmSync(scratch, { recursive: true, force: true }));
  const custom = join(scratch, 'my tmp dir');
  const record = join(scratch, 'observed');
  mkdirSync(custom);

  const observe = `
node() { :; }
jq() { :; }
cargo() { :; }
locus() {
  if [[ "${'$'}{1:-} ${'$'}{2:-} ${'$'}{3:-}" == "agent report --json" ]]; then
    printf '%s\\n' "$PWD" "$(cat .mcp.json)" > "$RECORD"
    exit 73
  fi
}
export -f node jq cargo locus
exec bash "$1"
`;
  const result = spawnSync('bash', ['-c', observe, 'proj-test', script], {
    env: { PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: custom, RECORD: record },
    encoding: 'utf8',
  });
  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  const [cwd, mcp] = readFileSync(record, 'utf8').split('\n');
  assert.equal(dirname(cwd), custom, 'SMOKE_PROJ must be created directly beneath the spaced TMPDIR');
  assert.match(basename(cwd), /^locus-hub-int-proj\.[^/]+$/);
  assert.equal(JSON.parse(mcp).mcpServers.locus.command, 'locus-mcp');
  assert.equal(existsSync(cwd), false, 'SMOKE_PROJ must be removed on exit');
  assert.deepEqual(readdirSync(custom), []);
});

// A relative TMPDIR is resolved against the caller's cwd, but the script cd's
// into the smoke project before exiting; the EXIT trap must still find and
// remove both directories.
test('relative TMPDIR holds both directories and they are removed after the script changes directory', (t) => {
  const scratch = mkdtempSync('/tmp/locus-tmpdir-rel-');
  t.after(() => rmSync(scratch, { recursive: true, force: true }));
  mkdirSync(join(scratch, 'rel parent'));
  writeFileSync(join(scratch, 'rel parent', 'keep'), 'sentinel');
  const record = join(scratch, 'observed');

  // Fail at the first command that runs after `cd "$SMOKE_PROJ"`.
  const observe = `
node() { :; }
jq() { :; }
cargo() { :; }
locus() {
  if [[ "$1" == status ]]; then
    printf '%s\\n' "$LOCUS_HOME" "$PWD" > "$RECORD"
    return 73
  fi
}
export -f node jq cargo locus
exec bash "$1"
`;
  const result = spawnSync('bash', ['-c', observe, 'relative-test', script], {
    cwd: scratch,
    env: { PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: 'rel parent', RECORD: record },
    encoding: 'utf8',
  });
  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 73, 'the injected failure must propagate');
  const paths = readFileSync(record, 'utf8').trim().split('\n');
  assert.equal(paths.length, 2);
  assert.match(basename(paths[0]), /^locus-hub-int\.[^/]+$/);
  assert.match(basename(paths[1]), /^locus-hub-int-proj\.[^/]+$/);
  for (const path of paths) {
    assert.ok(path.startsWith('/'), 'directory paths must be absolute');
    assert.equal(basename(dirname(path)), 'rel parent');
    assert.equal(existsSync(path), false, 'each directory must be removed on exit');
  }
  assert.deepEqual(readdirSync(join(scratch, 'rel parent')), ['keep']);
});
