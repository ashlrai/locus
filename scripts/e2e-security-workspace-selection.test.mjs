// Run: node --test scripts/e2e-security-workspace-selection.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { lstatSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, dirname, join } from 'node:path';
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

// ── Section 24 broken-link probes ───────────────────────────────────────────
// The verbatim probe block: the live SECURITY_WS receives its .locus.toml
// symlink, pin/run must fail closed inside it, and --json doctor must report
// UNSAFE — then the ok line prints and the EXIT trap removes the populated
// workspace. LOCUS_BIN is a private fixture that accepts only the three
// exact probe argv shapes and requires the broken link in the working
// directory, the same precondition the real binary fails closed on.
function brokenLinkProbeBlock() {
  const source = readFileSync(script, 'utf8');
  const section24 = source.indexOf('log "24.');
  const start = source.indexOf('ln -s missing-policy.toml "$SECURITY_WS/.locus.toml"', section24);
  const endMarker = 'ok "broken workspace link blocks force/run and makes doctor UNSAFE"';
  const end = source.indexOf(endMarker, start);
  assert.ok(section24 >= 0 && start > section24 && end > start,
    'e2e.sh must contain the section 24 broken-link probe block');
  return source.slice(start, end + endMarker.length);
}

test('section 24 broken-link probes use the live workspace and clean it up', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-secws-probe-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'controlled tmp');
  const bin = join(root, 'bin');
  const mainHome = join(root, 'main home');
  const wsCreated = join(root, 'security-ws-created');
  const doctorUsed = join(root, 'security-ws-doctor-used');
  const argvLog = join(root, 'locus-argv.log');
  mkdirSync(scratch);
  mkdirSync(bin);
  mkdirSync(mainHome);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  writeFileSync(join(bin, 'locus'), `#!/bin/bash
printf '%s\\t%s\\n' "$PWD" "$*" >> "$LOCUS_ARGV_LOG"
case "$*" in
  "pin env-probe --force" | "run -b env-probe --force -- true")
    if [[ "\${1:-}" == pin ]]; then
      [[ "$#" -eq 3 && "$1" == pin && "$2" == env-probe && "$3" == --force ]] || exit 98
    else
      [[ "$#" -eq 6 && "$1" == run && "$2" == -b && "$3" == env-probe && "$4" == --force && "$5" == -- && "$6" == true ]] || exit 98
    fi
    [[ -L .locus.toml ]] || exit 98
    printf 'workspace policy is broken or unreadable\\n' >&2
    exit 1
    ;;
  "--json doctor")
    [[ "$#" -eq 2 && "$1" == --json && "$2" == doctor ]] || exit 99
    [[ -L .locus.toml ]] || exit 99
    printf '%s' "$PWD" > "$DOCTOR_USED_RECORD"
    touch cleanup-sentinel || exit 97
    printf '%s\\n' '{"verdict":"UNSAFE","ok":false,"workspace":{"valid":false}}'
    exit 2
    ;;
  *) exit 96 ;;
esac
`, { mode: 0o755 });

  const collapsed = spawnSync(join(bin, 'locus'), ['--json doctor'], {
    cwd: mainHome, env: { ...process.env, LOCUS_ARGV_LOG: argvLog,
      DOCTOR_USED_RECORD: doctorUsed }, encoding: 'utf8', timeout: 15_000,
  });
  assert.ifError(collapsed.error);
  assert.equal(collapsed.signal, null);
  assert.equal(collapsed.status, 99, 'the fixture must reject a collapsed two-token argv');
  assert.throws(() => readFileSync(doctorUsed), { code: 'ENOENT' },
    'a rejected fixture call cannot record doctor use');
  writeFileSync(argvLog, '');

  const { cleanup, createHome, createWs } = securityLifecycle();
  const result = spawnSync('bash', ['-c', `set -euo pipefail
${cleanup}
${createHome}
${createWs}
log() { :; }
ok() { printf 'ok %s\\n' "$*"; }
die() { printf 'FAIL %s\\n' "$*" >&2; exit 1; }
LOCUS_HOME="$TEST_HOME"
LOCUS_BIN="$FAKE_LOCUS"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
trap 'e2e_cleanup' EXIT
${section24Selection()}
printf '%s' "$SECURITY_WS" > "$WS_CREATED"
${brokenLinkProbeBlock()}
`], {
    timeout: 15_000,
    env: { ...process.env, TMPDIR: scratch, TEST_HOME: mainHome,
      FAKE_LOCUS: join(bin, 'locus'), WS_CREATED: wsCreated,
      DOCTOR_USED_RECORD: doctorUsed, LOCUS_ARGV_LOG: argvLog },
    encoding: 'utf8',
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, `the probe block must succeed: ${result.stderr}`);
  assert.match(result.stdout, /ok security directories are created directly beneath TMPDIR/);
  assert.match(result.stdout, /ok broken workspace link blocks force\/run and makes doctor UNSAFE/);
  const workspace = readFileSync(wsCreated, 'utf8');
  assert.equal(dirname(workspace), scratch, 'the probe workspace sits beneath the controlled TMPDIR');
  assert.match(workspace, /\/locus-security-ws\.[^/\s]+$/);
  const calls = readFileSync(argvLog, 'utf8').trim().split('\n').map((line) => line.split('\t'));
  assert.deepEqual(calls.map(([, argv]) => argv), [
    'pin env-probe --force',
    'run -b env-probe --force -- true',
    '--json doctor',
  ], 'the block must issue exactly the three probe commands, in order');
  for (const [pwd] of calls) {
    assert.equal(pwd, workspace, 'every probe must run inside the live security workspace');
  }
  assert.equal(readFileSync(doctorUsed, 'utf8'), workspace,
    'security doctor used a different workspace');
  assert.throws(() => lstatSync(workspace), { code: 'ENOENT' },
    'security workspace and its contents must be removed on exit');
  assert.deepEqual(readdirSync(scratch), ['keep'], 'the controlled TMPDIR keeps only its own entries');
});

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

// ── Repeated SECURITY_WS creation ───────────────────────────────────────────
// The unconditional repeated-creation block at the tail of section 24 is
// extracted verbatim (not copied) so its own containment, distinctness and
// removal checks are the ones under test. The probes below run it under the
// real EXIT trap with SECURITY_WS already naming a live probe workspace —
// the block must stash and restore it, so the trap removes the live
// workspace and never the pair the block already removed.
function repeatedWorkspaceBlock() {
  const source = readFileSync(script, 'utf8');
  const start = source.indexOf('security_ws_live="$SECURITY_WS"');
  const endMarker = 'ok "two security workspaces are distinct and live directly beneath the temp root"';
  const end = source.indexOf(endMarker, start);
  assert.ok(start >= 0 && end > start, 'e2e.sh must contain the repeated security-workspace block');
  const block = source.slice(start, end + endMarker.length);
  assert.ok(block.includes('security_ws_root="${TMPDIR:-/tmp}"'),
    'the block must select the temporary root itself');
  assert.ok(block.includes('SECURITY_WS="$security_ws_live"'),
    'the block must restore the live workspace it stashed');
  return block;
}

// Runs the extracted block verbatim under the real EXIT trap, the same way
// section 24 reaches it after the security probes. SECURITY_WS names a live
// populated workspace when the block starts, as it does in the real script.
function repeatWorkspaceSection(t, { tempRoot, shim = '', extraEnv = {} }) {
  const dir = mkdtempSync(join(tmpdir(), 'locus-secws-repeat-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const mainHome = join(dir, 'main home');
  const liveWs = join(dir, 'live workspace');
  mkdirSync(mainHome);
  mkdirSync(liveWs);
  writeFileSync(join(liveWs, 'probe-marker'), 'live');
  const { cleanup, createWs } = securityLifecycle();
  const env = { ...process.env, MAIN_HOME: mainHome, LIVE_WS: liveWs, ...extraEnv };
  if (tempRoot === undefined) delete env.TMPDIR;
  else env.TMPDIR = tempRoot;
  const result = spawnSync('bash', ['-c', `set -euo pipefail
${cleanup}
${shim}
${createWs}
log() { :; }
ok() { printf 'ok %s\\n' "$*"; }
die() { printf 'FAIL %s\\n' "$*" >&2; exit 1; }
LOCUS_HOME="$MAIN_HOME"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" HTTP_MCP_PID="" MT_MCP_PID=""
SECURITY_WS="$LIVE_WS"
trap 'e2e_cleanup' EXIT
${repeatedWorkspaceBlock()}
`], { env, encoding: 'utf8' });
  return { result, dir, liveWs };
}

test('the verbatim block keeps both repeated workspaces beneath a controlled TMPDIR', (t) => {
  const dir = mkdtempSync(join(tmpdir(), 'locus-secws-repeat-run-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const scratch = join(dir, 'controlled root');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');

  const { result, dir: repeatDir, liveWs } = repeatWorkspaceSection(t, { tempRoot: scratch });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout,
    /ok two security workspaces are distinct and live directly beneath the temp root/);
  assert.deepEqual(readdirSync(scratch), ['keep'],
    'the block must remove both workspaces it created');
  assert.throws(() => lstatSync(liveWs), { code: 'ENOENT' },
    'the restored live workspace must be removed by the exit trap');
  assert.deepEqual(readdirSync(repeatDir), [],
    'the exit trap removes the suite home and the restored live workspace only');
});

// A TMPDIR may end in a slash (macOS hands out such values); the block strips
// it from both the selected root and each parent before comparing, so a bare
// equality check would regress here.
test('the verbatim block honors a controlled TMPDIR with a trailing slash', (t) => {
  const dir = mkdtempSync(join(tmpdir(), 'locus-secws-repeat-slash-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const scratch = join(dir, 'controlled root');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');

  const { result, liveWs } = repeatWorkspaceSection(t, { tempRoot: `${scratch}/` });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /ok two security workspaces are distinct/);
  assert.deepEqual(readdirSync(scratch), ['keep']);
  assert.throws(() => lstatSync(liveWs), { code: 'ENOENT' });
});

// A mktemp regression that relocates the repeated workspaces must stop the
// suite at the block's own containment check. The misplaced pair is left
// beneath the wrong root (the script header documents leftover temp dirs on
// failure) while the supplied root stays untouched and the restored live
// workspace is still removed by the exit trap.
test('repeated workspaces created outside the controlled TMPDIR stop the block', (t) => {
  const dir = mkdtempSync(join(tmpdir(), 'locus-secws-repeat-misplaced-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const scratch = join(dir, 'controlled root');
  const wrongRoot = join(dir, 'wrong root');
  mkdirSync(scratch);
  mkdirSync(wrongRoot);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  const shim = `mktemp() {
  if [[ "\${2:-}" == */locus-security-ws.XXXXXX ]]; then
    command mktemp -d "$WRONG_ROOT/locus-security-ws.XXXXXX"
    return
  fi
  command mktemp "$@"
}
`;

  const { result, liveWs } = repeatWorkspaceSection(t, {
    tempRoot: scratch, shim, extraEnv: { WRONG_ROOT: wrongRoot },
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 1, 'the containment check must fail the suite');
  assert.match(result.stderr,
    /FAIL repeated security workspaces were not created directly beneath the temp root/);
  assert.doesNotMatch(result.stdout, /ok two security workspaces are distinct/);
  const misplaced = readdirSync(wrongRoot);
  assert.equal(misplaced.length, 2, 'both misplaced workspaces stay beneath the wrong root');
  assert.ok(misplaced.every((n) => n.startsWith('locus-security-ws.')));
  assert.deepEqual(readdirSync(scratch), ['keep']);
  assert.throws(() => lstatSync(liveWs), { code: 'ENOENT' },
    'the restored live workspace must still be removed by the exit trap');
});

// A regression that hands back one directory twice must die at the
// distinctness check before either workspace is removed.
test('a repeated creation returning the same directory stops the block', (t) => {
  const dir = mkdtempSync(join(tmpdir(), 'locus-secws-repeat-dup-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const scratch = join(dir, 'controlled root');
  const seen = join(dir, 'seen-path');
  mkdirSync(scratch);
  const shim = `mktemp() {
  if [[ "\${2:-}" == */locus-security-ws.XXXXXX ]]; then
    [[ -s "$SEEN_PATH" ]] || command mktemp -d "$2" > "$SEEN_PATH"
    cat "$SEEN_PATH"
    return
  fi
  command mktemp "$@"
}
`;

  const { result, liveWs } = repeatWorkspaceSection(t, {
    tempRoot: scratch, shim, extraEnv: { SEEN_PATH: seen },
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 1, 'the distinctness check must fail the suite');
  assert.match(result.stderr,
    /FAIL repeated security workspace creation returned the same directory/);
  const dup = readFileSync(seen, 'utf8').trim();
  assert.equal(dirname(dup), scratch);
  assert.deepEqual(readdirSync(scratch), [basename(dup)],
    'only the duplicated workspace is left beneath the supplied root');
  assert.throws(() => lstatSync(liveWs), { code: 'ENOENT' });
});
