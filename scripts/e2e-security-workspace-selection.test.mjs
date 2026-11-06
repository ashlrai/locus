// Run: node --test scripts/e2e-security-workspace-selection.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { lstatSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'e2e.sh');

test('SECURITY_WS is created directly beneath the configured TMPDIR and cleaned up', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-security-ws-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'custom tmp [*]');
  const mainHome = join(root, 'main home');
  const observed = join(root, 'workspace-path');
  const template = join(root, 'template');
  mkdirSync(scratch);
  mkdirSync(mainHome);
  writeFileSync(join(scratch, 'keep'), 'unrelated');

  const source = readFileSync(script, 'utf8');
  const cleanup = source.match(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m)?.[0];
  const create = source.match(/^create_security_workspace\(\) \{[\s\S]*?^\}/m)?.[0];
  assert.ok(cleanup && create, 'e2e.sh must define the security workspace lifecycle');
  const result = spawnSync('bash', ['-c', `set -euo pipefail
${cleanup}
${create}
mktemp() {
  [[ "$#" == 2 && "$1" == -d ]] || exit 70
  printf '%s' "$2" > "$TEST_TEMPLATE"
  command mktemp "$@"
}
LOCUS_HOME="$TEST_HOME"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
trap 'e2e_cleanup' EXIT
create_security_workspace
[[ -d "$SECURITY_WS" && "\${SECURITY_WS%/*}" == "$TMPDIR" ]] || exit 71
printf 'fixture' > "$SECURITY_WS/probe"
[[ "$(cat "$SECURITY_WS/probe")" == fixture ]] || exit 72
printf '%s' "$SECURITY_WS" > "$TEST_OBSERVED"
`], {
    env: { ...process.env, TMPDIR: scratch, TEST_HOME: mainHome, TEST_TEMPLATE: template, TEST_OBSERVED: observed },
    encoding: 'utf8',
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(readFileSync(template, 'utf8'), `${scratch}/locus-security-ws.XXXXXX`);
  const workspace = readFileSync(observed, 'utf8');
  assert.equal(dirname(workspace), scratch);
  assert.match(workspace, /\/locus-security-ws\.[A-Za-z0-9]{6}$/);
  assert.throws(() => lstatSync(workspace), { code: 'ENOENT' });
  assert.deepEqual(readdirSync(scratch), ['keep']);
  assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'unrelated');
});

// The script's own section 2e rehearsal: SECURITY_WS beneath a supplied
// temporary root whose name contains spaces (including a doubled space). The
// probes run the block verbatim; stubs provide only log/ok/die and, where
// noted, a mktemp that misplaces the directory.
function section2e() {
  const source = readFileSync(script, 'utf8');
  const start = source.indexOf('log "2e.');
  const endMarker = 'ok "SECURITY_WS creation succeeds beneath a TMPDIR containing spaces"';
  const end = source.indexOf(endMarker, start);
  assert.ok(start >= 0 && end > start, 'e2e.sh must contain the section 2e rehearsal');
  return source.slice(start, end + endMarker.length);
}

function rehearse2e(t, shim = '') {
  const root = mkdtempSync(join(tmpdir(), 'locus-security-ws-2e-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const source = readFileSync(script, 'utf8');
  const cleanup = source.match(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m)?.[0];
  const create = source.match(/^create_security_workspace\(\) \{[\s\S]*?^\}/m)?.[0];
  assert.ok(cleanup && create, 'e2e.sh must define the security workspace lifecycle');
  const home = join(root, 'main home');
  mkdirSync(home);
  const result = spawnSync('bash', ['-c', `set -euo pipefail
${cleanup}
${create}
${shim}
log() { :; }
ok() { printf 'ok %s\\n' "$*"; }
die() { printf 'FAIL %s\\n' "$*" >&2; exit 1; }
LOCUS_HOME="$1"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
${section2e()}
`, 'security-ws-2e', home], { env: { ...process.env, TMPDIR: root }, encoding: 'utf8' });
  return { result, home, root };
}

test('section 2e creates SECURITY_WS under a spaced TMPDIR and cleans it up', (t) => {
  const { result, home, root } = rehearse2e(t);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /ok SECURITY_WS creation succeeds beneath a TMPDIR containing spaces/);
  const nested = join(home, 'security  ws temp root');
  const entries = readdirSync(nested).sort();
  // The rehearsed workspace and the subshell home are gone; only the path record remains.
  assert.deepEqual(entries, ['security-workspace-path']);
  const observed = readFileSync(join(nested, 'security-workspace-path'), 'utf8');
  assert.equal(dirname(observed), nested);
  assert.match(observed, /\/locus-security-ws\.[A-Za-z0-9]{6}$/);
  assert.throws(() => lstatSync(observed), { code: 'ENOENT' });
  assert.ok(lstatSync(root).isDirectory());
});

test('section 2e fails when SECURITY_WS is not created under the requested parent', (t) => {
  const { result } = rehearse2e(t, `mktemp() { command mktemp -d "\${TEST_ELSEWHERE:-/tmp}/locus-security-ws.XXXXXX"; }
TEST_ELSEWHERE="$(command mktemp -d)"`);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /FAIL SECURITY_WS was not created beneath the space-containing TMPDIR/);
});

function createFn() {
  const create = readFileSync(script, 'utf8').match(/^create_security_workspace\(\) \{[\s\S]*?^\}/m)?.[0];
  assert.ok(create, 'e2e.sh must define create_security_workspace');
  return create;
}

test('SECURITY_WS falls back to /tmp when TMPDIR is unset or empty', (t) => {
  for (const mode of ['unset', 'empty']) {
    const env = { ...process.env };
    if (mode === 'unset') delete env.TMPDIR; else env.TMPDIR = '';
    const result = spawnSync('bash', ['-c', `set -euo pipefail
${createFn()}
create_security_workspace
trap 'rm -rf "$SECURITY_WS"' EXIT
printf '%s' "$SECURITY_WS"
`], { env, encoding: 'utf8' });
    assert.equal(result.status, 0, `${mode}: ${result.stderr}`);
    assert.match(result.stdout, /^\/tmp\/locus-security-ws\.[A-Za-z0-9]{6}$/, mode);
    assert.throws(() => lstatSync(result.stdout), { code: 'ENOENT' }, mode);
  }
});

test('SECURITY_WS is only ever assigned through create_security_workspace', () => {
  const lines = readFileSync(script, 'utf8').split('\n');
  const assigns = lines.filter((l) => /^\s*SECURITY_WS=("\$\(|\$\()/.test(l));
  assert.equal(assigns.length, 1, `unexpected SECURITY_WS creations: ${assigns.join(' | ')}`);
  assert.match(assigns[0], /mktemp -d "\$\{TMPDIR:-\/tmp\}\/locus-security-ws\.XXXXXX"/);
});

// The script's own section 24 fixture selection: create_security_home and
// create_security_workspace run under the caller's TMPDIR, then a containment
// check stops the suite before the probes populate fixtures outside it. The
// probes below run that block verbatim; stubs provide only log/ok/die and,
// where noted, a recording or relocating mktemp.
function securityLifecycle() {
  const source = readFileSync(script, 'utf8');
  const cleanup = source.match(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m)?.[0];
  const createHome = source.match(/^create_security_home\(\) \{[\s\S]*?^\}/m)?.[0];
  const createWs = source.match(/^create_security_workspace\(\) \{[\s\S]*?^\}/m)?.[0];
  assert.ok(cleanup && createHome && createWs, 'e2e.sh must define the security directory lifecycle');
  return { cleanup, createHome, createWs };
}

function section24Selection() {
  const source = readFileSync(script, 'utf8');
  const start = source.indexOf('log "24.');
  const endMarker = 'ok "security directories are created directly beneath TMPDIR"\nfi';
  const end = source.indexOf(endMarker, start);
  assert.ok(start >= 0 && end > start, 'e2e.sh must contain the section 24 security selection block');
  return source.slice(start, end + endMarker.length);
}

function rehearse24(t, { tempRoot, shim = '', tail = '', extraEnv = {} }) {
  const root = mkdtempSync(join(tmpdir(), 'locus-secws-24-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const mainHome = join(root, 'main home');
  mkdirSync(mainHome);
  const { cleanup, createHome, createWs } = securityLifecycle();
  const env = { ...process.env, TEST_HOME: mainHome, ...extraEnv };
  if (tempRoot === undefined) delete env.TMPDIR;
  else env.TMPDIR = tempRoot;
  const result = spawnSync('bash', ['-c', `set -euo pipefail
${cleanup}
${shim}
${createHome}
${createWs}
log() { :; }
ok() { printf 'ok %s\\n' "$*"; }
die() { printf 'FAIL %s\\n' "$*" >&2; exit 1; }
LOCUS_HOME="$TEST_HOME"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
trap 'e2e_cleanup' EXIT
${section24Selection()}
${tail}
`, 'security-24', mainHome], { env, encoding: 'utf8' });
  return { result, root };
}

test('section 24 creates the security workspace directly beneath a controlled TMPDIR', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-secws-24-run-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'fixture root [*]');
  const observed = join(root, 'observed-paths');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');

  const { result } = rehearse24(t, {
    tempRoot: scratch,
    extraEnv: { OBSERVED: observed },
    tail: `
# Use the workspace the way the probes do: a policy link must land inside it.
ln -s missing-policy.toml "$SECURITY_WS/.locus.toml" || exit 71
[[ -L "$SECURITY_WS/.locus.toml" && \\
  "$(cd "$SECURITY_WS" && pwd -P)" == "$(cd "$TMPDIR" && pwd -P)/\${SECURITY_WS##*/}" ]] || exit 72
printf '%s\\n%s' "$SECURITY_HOME" "$SECURITY_WS" > "$OBSERVED"
`,
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /ok security directories are created directly beneath TMPDIR/);
  const [home, workspace] = readFileSync(observed, 'utf8').split('\n');
  assert.equal(dirname(home), scratch);
  assert.match(home, /\/locus-security-e2e\.[^/\s]+$/);
  assert.equal(dirname(workspace), scratch, 'the workspace sits directly beneath the controlled TMPDIR');
  assert.match(workspace, /\/locus-security-ws\.[^/\s]+$/);
  for (const dir of [home, workspace]) {
    assert.throws(() => lstatSync(dir), { code: 'ENOENT' }, 'fixtures must be removed on exit');
  }
  assert.deepEqual(readdirSync(scratch), ['keep'], 'the controlled TMPDIR keeps only its own entries');
});

test('a security workspace created outside the controlled TMPDIR stops section 24', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-secws-24-misplaced-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'controlled tmp');
  const wrongRoot = join(root, 'wrong root');
  const created = join(root, 'ws-created');
  mkdirSync(scratch);
  mkdirSync(wrongRoot);
  // Answer with the expected name shape but relocate the workspace outside
  // TMPDIR; the containment check must stop the suite and the exit trap must
  // still remove the misplaced directory.
  const shim = `mktemp() {
  if [[ "\${2:-}" == */locus-security-ws.XXXXXX ]]; then
    d="$(command mktemp -d "$WRONG_ROOT/locus-security-ws.XXXXXX")"
    printf '%s' "$d" > "$WS_CREATED"
    printf '%s\\n' "$d"
    return
  fi
  command mktemp "$@"
}
`;

  const { result } = rehearse24(t, {
    tempRoot: scratch,
    shim,
    extraEnv: { WRONG_ROOT: wrongRoot, WS_CREATED: created },
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 1, 'the containment check must fail the suite');
  assert.match(result.stderr, /FAIL security directories were not created directly beneath TMPDIR/);
  assert.doesNotMatch(result.stdout, /ok security directories are created directly beneath TMPDIR/);
  const workspace = readFileSync(created, 'utf8');
  assert.equal(dirname(workspace), wrongRoot);
  assert.match(workspace, /\/locus-security-ws\.[^/]+$/);
  assert.throws(() => lstatSync(workspace), { code: 'ENOENT' },
    'the misplaced workspace must be removed by the exit trap');
  assert.deepEqual(readdirSync(wrongRoot), []);
  assert.deepEqual(readdirSync(scratch), [], 'the correctly placed security home must be removed too');
});

for (const mode of ['unset', 'empty']) {
  test(`section 24 skips the containment check when TMPDIR is ${mode}`, (t) => {
    const root = mkdtempSync(join(tmpdir(), 'locus-secws-24-fallback-'));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const controlled = join(root, 'controlled');
    const templates = join(root, 'templates');
    mkdirSync(controlled);
    // Record the requested templates and redirect creation into a controlled
    // directory so the probe never writes to the real /tmp.
    const shim = `mktemp() {
  printf '%s\\n' "\${2:-}" >> "$TEST_TEMPLATES"
  if [[ "\${2:-}" == /tmp/locus-security-*.XXXXXX ]]; then
    command mktemp -d "$CONTROLLED/$(basename "\${2:-}")"
    return
  fi
  command mktemp "$@"
}
`;

    const { result } = rehearse24(t, {
      tempRoot: mode === 'unset' ? undefined : '',
      shim,
      extraEnv: { CONTROLLED: controlled, TEST_TEMPLATES: templates },
    });

    assert.equal(result.signal, null);
    assert.equal(result.status, 0, `${mode}: ${result.stderr}`);
    assert.doesNotMatch(result.stdout, /ok security directories are created directly beneath TMPDIR/,
      `the supplied-TMPDIR confirmation must not print when TMPDIR is ${mode}`);
    assert.deepEqual(readFileSync(templates, 'utf8').trim().split('\n'), [
      '/tmp/locus-security-e2e.XXXXXX',
      '/tmp/locus-security-ws.XXXXXX',
    ], `the script must select the /tmp root when TMPDIR is ${mode}`);
    assert.deepEqual(readdirSync(controlled), [], 'the redirected fixtures must be cleaned up');
  });
}

// ── Temporary-root selection edges ──────────────────────────────────────────
// The selection contract is ${TMPDIR:-/tmp}: a supplied TMPDIR is the
// containment boundary the caller chose. The probes below run
// create_security_workspace and e2e_cleanup verbatim through the edges the
// happy-path checks above do not reach — an unusable root, an inherited
// SECURITY_WS value, and the trailing-slash TMPDIR macOS hands out.

// Runs create_security_workspace under the real EXIT trap inside `root`,
// records the selected path on success, and returns the spawn result. The
// reset line mirrors the script's own initialization before fixture creation;
// the trap's removal of LOCUS_HOME doubles as a cleanup witness.
function selectWorkspace({ root, tmpdir, tail = '', extraEnv = {} }) {
  const observed = join(root, 'observed-workspace');
  const mainHome = join(root, 'locus home');
  mkdirSync(mainHome);
  const { cleanup, createWs } = securityLifecycle();
  const env = { ...process.env, MAIN_HOME: mainHome, OBSERVED: observed, ...extraEnv };
  if (tmpdir === undefined) delete env.TMPDIR;
  else env.TMPDIR = tmpdir;
  const result = spawnSync('bash', ['-c', `set -euo pipefail
${cleanup}
${createWs}
LOCUS_HOME="$MAIN_HOME"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
trap 'e2e_cleanup' EXIT
create_security_workspace
${tail}
printf '%s' "$SECURITY_WS" > "$OBSERVED"
`], { env, encoding: 'utf8' });
  return { observed, result };
}

// A supplied TMPDIR must name an existing directory: a missing one — or a
// plain file standing in its place — fails loudly instead of silently
// selecting a different root, and nothing is created in its place.
for (const mode of ['missing', 'a plain file']) {
  test(`a supplied TMPDIR that is ${mode} fails selection without creating anything`, (t) => {
    const root = mkdtempSync(join(tmpdir(), 'locus-secws-bad-root-'));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const scratch = join(root, 'supplied tmp');
    if (mode === 'a plain file') writeFileSync(scratch, 'not a directory');

    const { observed, result } = selectWorkspace({ root, tmpdir: scratch });

    assert.equal(result.signal, null);
    assert.notEqual(result.status, 0, `creation under ${mode} TMPDIR must fail`);
    assert.match(result.stderr, /mktemp/, 'the failure must come from the allocation itself');
    assert.throws(() => lstatSync(observed), { code: 'ENOENT' }, 'no workspace may be recorded');
    if (mode === 'missing') {
      assert.throws(() => lstatSync(scratch), { code: 'ENOENT' }, 'the missing root must not be created');
      assert.deepEqual(readdirSync(root), [], 'the exit trap still removed the suite home');
    } else {
      assert.equal(readFileSync(scratch, 'utf8'), 'not a directory', 'the plain file is untouched');
      assert.deepEqual(readdirSync(root), ['supplied tmp'], 'the exit trap still removed the suite home');
    }
  });
}

// An inherited SECURITY_WS value must not change the selected root: the
// workspace is still allocated beneath the supplied TMPDIR, and the inherited
// directory survives the exit trap untouched.
test('an inherited SECURITY_WS does not change the selected root', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-secws-inherited-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'custom tmp');
  const inherited = join(root, 'inherited workspace');
  mkdirSync(scratch);
  mkdirSync(inherited);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  writeFileSync(join(inherited, 'keep'), 'inherited');

  const { observed, result } = selectWorkspace({
    root, tmpdir: scratch, extraEnv: { SECURITY_WS: inherited },
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, result.stderr);
  const workspace = readFileSync(observed, 'utf8');
  assert.equal(dirname(workspace), scratch,
    'the selected root is the supplied TMPDIR, not the inherited workspace');
  assert.notEqual(workspace, inherited);
  assert.match(workspace, /\/locus-security-ws\.[^/\s]+$/);
  assert.throws(() => lstatSync(workspace), { code: 'ENOENT' },
    'the selected workspace must be removed on exit');
  assert.equal(readFileSync(join(inherited, 'keep'), 'utf8'), 'inherited',
    'the inherited workspace must survive untouched');
  assert.deepEqual(readdirSync(scratch), ['keep'], 'the supplied TMPDIR keeps only its own entries');
});

// macOS hands out a TMPDIR with a trailing slash. The workspace must still
// land directly beneath that root, and the containment comparison
// (`${SECURITY_WS%/*}` equals "$TMPDIR") must keep holding for the
// unnormalized value.
test('a TMPDIR with a trailing slash still holds the security workspace', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-secws-slash-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'custom tmp');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');

  const { observed, result } = selectWorkspace({
    root,
    tmpdir: `${scratch}/`,
    tail: '[[ -d "$SECURITY_WS" && "${SECURITY_WS%/*}" == "$TMPDIR" ]] || exit 76',
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, result.stderr);
  const workspace = readFileSync(observed, 'utf8');
  assert.ok(workspace.startsWith(`${scratch}//locus-security-ws.`),
    'the supplied slash is kept verbatim and the workspace is a direct child of the root');
  assert.throws(() => lstatSync(workspace), { code: 'ENOENT' });
  assert.deepEqual(readdirSync(scratch), ['keep']);
});
