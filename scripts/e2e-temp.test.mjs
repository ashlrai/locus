// Run: node --test scripts/e2e-temp.test.mjs
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
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

test('security directory guide maps the script fixtures and their cleanup', () => {
  const source = readFileSync(script, 'utf8');
  const doc = readFileSync(join(root, 'docs', 'e2e-security-directories.md'), 'utf8');
  const rows = [...doc.matchAll(/^\| `(SECURITY_(?:HOME|WS))` \| `([^`]+)` \| ([^|\n]+) \|$/gm)];
  assert.deepEqual(rows.map(([, variable]) => variable), ['SECURITY_HOME', 'SECURITY_WS']);
  for (const [, variable, prefix, purpose] of rows) {
    assert.ok(purpose.trim(), `${variable} needs a purpose`);
    assert.ok(source.includes(`${variable}="$(mktemp -d "\${TMPDIR:-/tmp}/${prefix}XXXXXX")"`),
      `${variable} prefix must match its creation in the script`);
    assert.ok(source.includes(`"\${${variable}:-}"`), `${variable} must be covered by cleanup`);
  }
  assert.match(doc, /`LOCUS_HOME`/);
  assert.match(doc, /`\.locus\.toml` symlink/);
  assert.match(doc, /`EXIT` trap/);
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
  const scratch = join(fixture.root, 'custom tmp');
  const homeCreated = join(fixture.root, 'home-created');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  const realMktemp = execFileSync('which', ['mktemp'], { encoding: 'utf8' }).trim();
  writeFileSync(join(fixture.bin, 'mktemp'), `#!/bin/sh
case "$2" in
  */locus-e2e.XXXXXX)
    home="$("$REAL_MKTEMP" "$@")" || exit 1
    test -d "$home" || exit 1
    printf '%s' "$home" > "$HOME_CREATED"
    touch "$home/sentinel"
    printf '%s\n' "$home"
    ;;
  *) exec "$REAL_MKTEMP" "$@" ;;
esac
`, { mode: 0o755 });
  const result = run(fixture, scratch, { HOME_CREATED: homeCreated, REAL_MKTEMP: realMktemp });

  assert.notEqual(result.status, 0);
  assert.equal(result.signal, null);
  assert.match(result.stderr, /fresh quickstart failed/);
  const home = readFileSync(homeCreated, 'utf8');
  assert.equal(dirname(home), scratch);
  assert.match(home, /\/locus-e2e\.[^/]+$/);
  assert.equal(existsSync(home), false, 'home and its sentinel must be removed on exit');
  assert.deepEqual(readdirSync(scratch), ['keep']);
});

test('exit cleanup preserves an inherited graph home before the fallback runs', (t) => {
  const fixture = setup(t);
  const scratch = join(fixture.root, 'scratch');
  const inherited = join(fixture.root, 'unrelated graph home');
  mkdirSync(scratch);
  mkdirSync(inherited);
  writeFileSync(join(inherited, 'keep'), 'unrelated');

  const result = run(fixture, scratch, { GRAPH_HOME: inherited });

  assert.equal(result.status, 1);
  assert.match(result.stderr, /fresh quickstart failed/);
  assert.deepEqual(readdirSync(inherited), ['keep']);
  assert.deepEqual(readdirSync(scratch), []);
});

test('exit cleanup preserves inherited security directories before the probes run', (t) => {
  const fixture = setup(t);
  const scratch = join(fixture.root, 'scratch');
  const inheritedHome = join(fixture.root, 'unrelated security home');
  const inheritedWorkspace = join(fixture.root, 'unrelated security workspace');
  mkdirSync(scratch);
  for (const dir of [inheritedHome, inheritedWorkspace]) {
    mkdirSync(dir);
    writeFileSync(join(dir, 'keep'), 'unrelated');
  }

  const result = run(fixture, scratch, { SECURITY_HOME: inheritedHome, SECURITY_WS: inheritedWorkspace });

  assert.equal(result.status, 1);
  assert.match(result.stderr, /fresh quickstart failed/);
  for (const dir of [inheritedHome, inheritedWorkspace]) assert.deepEqual(readdirSync(dir), ['keep']);
  assert.deepEqual(readdirSync(scratch), []);
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

for (const exitStatus of [0, 73]) {
  test(`workspace honors TMPDIR and is removed on exit ${exitStatus}`, (t) => {
    const fixture = setup(t);
    const scratch = join(fixture.root, 'custom tmp');
    const record = join(fixture.root, 'workspace-path');
    mkdirSync(scratch);
    writeFileSync(join(scratch, 'keep'), 'unrelated');
    const source = readFileSync(script, 'utf8');
    const cleanup = source.match(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m)?.[0];
    const createWorkspace = source.match(/^WS_DIR="\$\(mktemp -d .*\)"$/m)?.[0];
    assert.ok(cleanup, 'missing e2e cleanup function');
    assert.ok(createWorkspace, 'missing workspace creation');
    const probe = `${cleanup}
TMPDIR="$1"
LOCUS_HOME="$TMPDIR/main-home"
GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
mkdir "$LOCUS_HOME"
trap 'e2e_cleanup' EXIT
${createWorkspace}
[[ -d "$WS_DIR" && "\${WS_DIR%/*}" == "$TMPDIR" && "\${WS_DIR##*/}" == locus-ws.* ]] || exit 1
printf '%s' "$WS_DIR" > "$2"
touch "$WS_DIR/cleanup-sentinel"
exit "$3"
`;
    const result = spawnSync('bash', ['-c', probe, 'workspace-cleanup', scratch, record, String(exitStatus)],
      { encoding: 'utf8' });
    assert.equal(result.status, exitStatus, 'workspace creation or cleanup probe failed');
    assert.equal(result.signal, null);
    const workspace = readFileSync(record, 'utf8');
    assert.equal(dirname(workspace), scratch);
    assert.match(workspace, /\/locus-ws\.[^/]+$/);
    assert.deepEqual(readdirSync(scratch), ['keep'], 'cleanup must remove workspace and preserve unrelated files');
  });
}

for (const exitStatus of [0, 73]) {
  test(`security fixture directories are removed on exit ${exitStatus}`, (t) => {
    const fixture = setup(t);
    const scratch = join(fixture.root, 'custom tmp');
    mkdirSync(scratch);
    writeFileSync(join(scratch, 'keep'), 'unrelated');
    const source = readFileSync(script, 'utf8');
    const cleanup = source.match(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m)?.[0];
    const createHome = source.match(/^create_security_home\(\) \{[\s\S]*?^\}/m)?.[0];
    const createWorkspace = source.match(/^create_security_workspace\(\) \{[\s\S]*?^\}/m)?.[0];
    assert.ok(cleanup, 'missing e2e cleanup function');
    assert.ok(createHome, 'missing security home creation function');
    assert.ok(createWorkspace, 'missing security workspace creation');
    const probe = `${cleanup}
${createHome}
${createWorkspace}
TMPDIR="$1"
LOCUS_HOME="$TMPDIR/main-home"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
mkdir "$LOCUS_HOME"
trap 'e2e_cleanup' EXIT
create_security_home
create_security_workspace
[[ -d "$SECURITY_HOME" && -d "$SECURITY_WS" && ! "$SECURITY_HOME" -ef "$SECURITY_WS" && "\${SECURITY_HOME%/*}" == "$TMPDIR" && "\${SECURITY_WS%/*}" == "$TMPDIR" ]] || exit 1
touch "$SECURITY_HOME/sentinel" "$SECURITY_WS/sentinel"
exit "$2"
`;
    const result = spawnSync('bash', ['-c', probe, 'security-cleanup', scratch, String(exitStatus)], { encoding: 'utf8' });
    assert.equal(result.status, exitStatus, 'security fixture creation or cleanup probe failed');
    assert.equal(result.signal, null);
    assert.deepEqual(readdirSync(scratch), ['keep'], 'cleanup must remove fixtures and preserve unrelated files');
  });
}
