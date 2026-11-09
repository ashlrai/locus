// Run: node --test scripts/e2e-workspace-selection.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { lstatSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const script = join(root, 'scripts', 'e2e.sh');

// The unit under test is the script's workspace lifecycle: create_workspace
// selects the temporary root (${TMPDIR:-/tmp}) and creates WS_DIR there, the
// section 4 block verifies the workspace sits directly beneath a supplied
// TMPDIR, and the e2e_cleanup EXIT trap removes it. The probes below run all
// three verbatim; shims provide only log/ok/die and, where noted, a recording
// or relocating mktemp. The last two tests run the script's own section 2d2
// rehearsal, which exercises the same selection beneath a plain custom TMPDIR.
function workspaceParts() {
  const source = readFileSync(script, 'utf8');
  const cleanup = source.match(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m)?.[0];
  const createWorkspace = source.match(/^create_workspace\(\) \{[\s\S]*?^\}/m)?.[0];
  assert.ok(cleanup && createWorkspace,
    'e2e.sh must contain the workspace lifecycle');
  return { cleanup, createWorkspace };
}

// The script's real selection block from section 4: create_workspace followed
// by the supplied-TMPDIR containment check, through the closing fi.
function selectionBlock() {
  const source = readFileSync(script, 'utf8');
  const start = source.indexOf('create_workspace\n# Same containment rule');
  const endMarker = 'ok "workspace is created directly beneath TMPDIR"\nfi';
  const end = source.indexOf(endMarker, start);
  assert.ok(start >= 0 && end > start,
    'e2e.sh must contain the WS_DIR temporary-root selection block');
  return source.slice(start, end + endMarker.length);
}

function fixture(t) {
  const dir = mkdtempSync(join(tmpdir(), 'locus-e2e-workspace-selection-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  return dir;
}

// Runs the selection block under the real EXIT trap. tmpdir === undefined
// leaves TMPDIR unset; extraEnv feeds shim record files. Returns the spawn
// result and the workspace path the probe writes before exit cleanup runs.
function select(t, { tmpdir, shim = '', tail = '', extraEnv = {} }) {
  const dir = fixture(t);
  const observed = join(dir, 'observed-workspace');
  const mainHome = join(dir, 'main-home');
  mkdirSync(mainHome);
  const { cleanup, createWorkspace } = workspaceParts();
  const env = { ...process.env, MAIN_HOME: mainHome, OBSERVED: observed, ...extraEnv };
  if (tmpdir === undefined) delete env.TMPDIR;
  else env.TMPDIR = tmpdir;
  const result = spawnSync('bash', ['-c', `set -euo pipefail
${cleanup}
${createWorkspace}
${shim}
log() { :; }
ok() { printf 'ok %s\\n' "$*"; }
die() { printf 'FAIL %s\\n' "$*" >&2; exit 1; }
LOCUS_HOME="$MAIN_HOME"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
trap 'e2e_cleanup' EXIT
${selectionBlock()}
${tail}
printf '%s' "$WS_DIR" > "$OBSERVED"
`], { env, encoding: 'utf8' });
  return { dir, observed, result };
}

test('WS_DIR is created under the supplied TMPDIR', (t) => {
  const dir = fixture(t);
  const scratch = join(dir, 'custom tmp');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');

  const { observed, result } = select(t, { tmpdir: scratch, tail: `
[[ -d "$WS_DIR" && -w "$WS_DIR" && -x "$WS_DIR" ]] || exit 71
mkdir -p "$WS_DIR/nested dir" || exit 72
printf 'payload' > "$WS_DIR/nested dir/f.toml" || exit 73
[[ "$(cat "$WS_DIR/nested dir/f.toml")" == payload ]] || exit 74
` });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /ok workspace is created directly beneath TMPDIR/,
    'the script must confirm the workspace landed beneath the supplied root');
  const workspace = readFileSync(observed, 'utf8');
  assert.equal(dirname(workspace), scratch, 'the workspace is created directly beneath TMPDIR');
  assert.match(workspace, /\/locus-ws\.[A-Za-z0-9]{6}$/, 'the mktemp prefix and suffix shape hold');
  assert.throws(() => lstatSync(workspace), { code: 'ENOENT' },
    'the populated workspace must be removed on exit');
  assert.deepEqual(readdirSync(scratch), ['keep'], 'cleanup must preserve unrelated files');
});

test('a supplied TMPDIR with spaces reaches mktemp inside one template argument', (t) => {
  const dir = fixture(t);
  const scratch = join(dir, 'scratch dir with  spaces');
  const argc = join(dir, 'argc');
  const template = join(dir, 'template');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  // A spaced parent must arrive inside one quoted template; record argv, then
  // let the real command create the directory it was given.
  const shim = `mktemp() {
  printf '%s' "$#" > "$MKTEMP_ARGC"
  printf '%s' "$2" > "$MKTEMP_TEMPLATE"
  command mktemp "$@"
}
`;

  const { observed, result } = select(t, {
    tmpdir: scratch, shim,
    extraEnv: { MKTEMP_ARGC: argc, MKTEMP_TEMPLATE: template },
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(readFileSync(argc, 'utf8'), '2', 'mktemp must receive exactly -d and one template');
  assert.equal(readFileSync(template, 'utf8'), `${scratch}/locus-ws.XXXXXX`,
    'the spaced root must arrive inside a single template argument');
  const workspace = readFileSync(observed, 'utf8');
  assert.equal(dirname(workspace), scratch);
  assert.match(workspace, /\/locus-ws\.[^/\s]+$/, 'the workspace name is one unsplit component');
  assert.throws(() => lstatSync(workspace), { code: 'ENOENT' });
  assert.deepEqual(readdirSync(scratch), ['keep']);
});

for (const [label, tmpdir] of [['unset', undefined], ['empty', '']]) {
  test(`the workspace root falls back to /tmp when TMPDIR is ${label}`, (t) => {
    const dir = fixture(t);
    const controlled = join(dir, 'controlled');
    const template = join(dir, 'template');
    mkdirSync(controlled);
    // Record the requested template and redirect creation into the controlled
    // directory so the probe never writes to the real /tmp.
    const shim = `mktemp() {
  if [[ "\${2:-}" == /tmp/locus-ws.XXXXXX ]]; then
    printf '%s' "$2" > "$MKTEMP_TEMPLATE"
    command mktemp -d "$CONTROLLED/locus-ws.XXXXXX"
    return
  fi
  command mktemp "$@"
}
`;

    const { observed, result } = select(t, {
      tmpdir, shim,
      extraEnv: { CONTROLLED: controlled, MKTEMP_TEMPLATE: template },
    });

    assert.equal(result.signal, null);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(readFileSync(template, 'utf8'), '/tmp/locus-ws.XXXXXX',
      `the script must select the /tmp root when TMPDIR is ${label}`);
    assert.doesNotMatch(result.stdout, /ok workspace is created directly beneath TMPDIR/,
      'the supplied-TMPDIR confirmation must not print without a supplied TMPDIR');
    assert.equal(dirname(readFileSync(observed, 'utf8')), controlled);
    assert.deepEqual(readdirSync(controlled), [], 'the redirected workspace must be cleaned up');
  });
}

test('an inherited WS_DIR does not change the selected root', (t) => {
  const dir = fixture(t);
  const scratch = join(dir, 'custom tmp');
  const inherited = join(dir, 'inherited workspace');
  mkdirSync(scratch);
  mkdirSync(inherited);
  writeFileSync(join(inherited, 'keep'), 'inherited');

  const { observed, result } = select(t, {
    tmpdir: scratch, extraEnv: { WS_DIR: inherited },
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, result.stderr);
  const workspace = readFileSync(observed, 'utf8');
  assert.equal(dirname(workspace), scratch,
    'the selected root is the supplied TMPDIR, not the inherited workspace');
  assert.notEqual(workspace, inherited);
  assert.throws(() => lstatSync(workspace), { code: 'ENOENT' });
  assert.equal(readFileSync(join(inherited, 'keep'), 'utf8'), 'inherited',
    'the inherited workspace must survive untouched');
});

// A mktemp regression that relocates the workspace must stop the suite at the
// selection check; the exit trap still removes the misplaced directory.
test('a workspace created outside the supplied TMPDIR fails the selection check', (t) => {
  const dir = fixture(t);
  const scratch = join(dir, 'custom tmp');
  const wrongRoot = join(dir, 'wrong root');
  const created = join(dir, 'ws-created');
  mkdirSync(scratch);
  mkdirSync(wrongRoot);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  const shim = `mktemp() {
  local d
  if [[ "\${2:-}" == */locus-ws.XXXXXX ]]; then
    d="$(command mktemp -d "$WRONG_ROOT/locus-ws.XXXXXX")"
    printf '%s' "$d" > "$WS_CREATED"
    printf '%s\\n' "$d"
    return
  fi
  command mktemp "$@"
}
`;

  const { result } = select(t, {
    tmpdir: scratch, shim,
    extraEnv: { WRONG_ROOT: wrongRoot, WS_CREATED: created },
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 1, 'the containment check must fail closed');
  assert.doesNotMatch(result.stdout, /ok workspace is created directly beneath TMPDIR/);
  assert.match(result.stderr, /FAIL WS_DIR was not created directly beneath TMPDIR/);
  const misplaced = readFileSync(created, 'utf8');
  assert.equal(dirname(misplaced), wrongRoot);
  assert.match(misplaced, /\/locus-ws\.[^/]+$/);
  assert.throws(() => lstatSync(misplaced), { code: 'ENOENT' },
    'the misplaced workspace must be removed on exit');
  assert.deepEqual(readdirSync(wrongRoot), []);
  assert.deepEqual(readdirSync(scratch), ['keep']);
});

// The script's own section 2d2 rehearsal: a subshell exports a plain custom
// TMPDIR inside the main home, creates the workspace beneath it, and relies
// on its EXIT trap. The probes below run the block verbatim; stubs provide
// log/ok/die and, where noted, a recording or relocating mktemp.
function section2d2() {
  const source = readFileSync(script, 'utf8');
  const start = source.indexOf('log "2d2.');
  const endMarker = 'ok "WS_DIR is created beneath a plain custom TMPDIR and cleaned on exit"';
  const end = source.indexOf(endMarker, start);
  assert.ok(start >= 0 && end > start, 'e2e.sh must contain the section 2d2 rehearsal');
  return source.slice(start, end + endMarker.length);
}

// Runs the real section 2d2 (not a copy of its subshell) against a prepared
// main home. The block owns the subshell trap itself, so the main home and
// the supplied temporary root stay inspectable after the spawn exits.
function rehearse2d2(home, { shim = '', extraEnv = {} } = {}) {
  const { cleanup, createWorkspace } = workspaceParts();
  return spawnSync('bash', ['-c', `set -euo pipefail
${cleanup}
${shim}
${createWorkspace}
log() { :; }
ok() { printf 'ok %s\\n' "$*"; }
die() { printf 'FAIL %s\\n' "$*" >&2; exit 1; }
LOCUS_HOME="$1"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
${section2d2()}
`, 'workspace-2d2', home], { env: { ...process.env, ...extraEnv }, encoding: 'utf8' });
}

test('section 2d2 creates WS_DIR beneath the plain custom TMPDIR and cleans it up', (t) => {
  const dir = fixture(t);
  const home = join(dir, 'main home');
  const outer = join(dir, 'outer tmp');
  const template = join(dir, 'mktemp-template');
  const argc = join(dir, 'mktemp-argc');
  mkdirSync(home);
  mkdirSync(outer);
  // Record the requested template and argument count, then let the real
  // command create the directory it was given. The outer TMPDIR stays visible
  // to prove the subshell-selected root is what reaches mktemp.
  const shim = `mktemp() {
  printf '%s' "$2" > "$MKTEMP_TEMPLATE"
  printf '%s' "$#" > "$MKTEMP_ARGC"
  command mktemp "$@"
}
`;

  const result = rehearse2d2(home, {
    shim,
    extraEnv: { TMPDIR: outer, MKTEMP_TEMPLATE: template, MKTEMP_ARGC: argc },
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, `the 2d2 rehearsal must succeed: ${result.stderr}`);
  assert.match(result.stdout,
    /ok WS_DIR is created beneath a plain custom TMPDIR and cleaned on exit/);
  const selected = join(home, 'ws-root');
  assert.equal(readFileSync(argc, 'utf8'), '2',
    'mktemp must receive exactly -d and one template');
  assert.equal(readFileSync(template, 'utf8'), `${selected}/locus-ws.XXXXXX`,
    'the subshell-selected root must reach mktemp inside one template argument');
  const recorded = readFileSync(join(selected, 'workspace-path'), 'utf8');
  assert.equal(dirname(recorded), selected,
    'the rehearsal workspace is created directly beneath the supplied TMPDIR');
  assert.match(recorded, /\/locus-ws\.[A-Za-z0-9]{6}$/,
    'the workspace name is one unsplit component');
  assert.throws(() => lstatSync(recorded), { code: 'ENOENT' },
    'the rehearsal workspace must be removed on subshell exit');
  assert.deepEqual(readdirSync(selected), ['workspace-path'],
    'only the recorded workspace path may remain beneath the supplied root');
  assert.deepEqual(readdirSync(outer), [],
    'the outer TMPDIR must not receive the workspace');
  assert.deepEqual(readdirSync(home), ['ws-root'],
    'the subshell trap must leave the main home and the supplied root intact');
});

// A mktemp regression that relocates the rehearsal workspace outside the
// supplied root must stop the suite at the 2d2 subshell; the subshell trap
// still removes the misplaced directory before the failure reaches the caller.
test('a rehearsal workspace created outside the plain TMPDIR stops section 2d2', (t) => {
  const dir = fixture(t);
  const home = join(dir, 'main home');
  const wrongRoot = join(dir, 'wrong root');
  const created = join(dir, 'ws-created');
  mkdirSync(home);
  mkdirSync(wrongRoot);
  const shim = `mktemp() {
  local d
  if [[ "\${2:-}" == */locus-ws.XXXXXX ]]; then
    d="$(command mktemp -d "$WRONG_ROOT/locus-ws.XXXXXX")"
    printf '%s' "$d" > "$WS_CREATED"
    printf '%s\\n' "$d"
    return
  fi
  command mktemp "$@"
}
`;

  const result = rehearse2d2(home, {
    shim,
    extraEnv: { WRONG_ROOT: wrongRoot, WS_CREATED: created },
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 1, 'the containment check must fail the suite');
  assert.match(result.stderr, /FAIL WS_DIR was not created beneath the plain custom TMPDIR/);
  assert.doesNotMatch(result.stdout, /ok WS_DIR is created/);
  const misplaced = readFileSync(created, 'utf8');
  assert.equal(dirname(misplaced), wrongRoot);
  assert.match(misplaced, /\/locus-ws\.[^/]+$/);
  assert.throws(() => lstatSync(misplaced), { code: 'ENOENT' },
    'the misplaced workspace must be removed by the subshell exit trap');
  assert.deepEqual(readdirSync(wrongRoot), []);
  assert.deepEqual(readdirSync(join(home, 'ws-root')), [],
    'the rehearsal exits before recording a workspace path');
});

test('WS_DIR is only ever assigned through create_workspace', () => {
  const lines = readFileSync(script, 'utf8').split('\n');
  const assigns = lines.filter((l) => /^\s*WS_DIR=("\$\(|\$\()/.test(l));
  assert.equal(assigns.length, 1, `unexpected WS_DIR creations: ${assigns.join(' | ')}`);
  assert.match(assigns[0], /mktemp -d "\$\{TMPDIR:-\/tmp\}\/locus-ws\.XXXXXX"/);
});

// The header documents that a nonempty TMPDIR must name an existing writable
// directory. A missing one must stop the suite rather than fall back to /tmp.
test('a nonexistent TMPDIR fails closed without creating a workspace', (t) => {
  const dir = fixture(t);
  const missing = join(dir, 'does not exist');

  const { observed, result } = select(t, { tmpdir: missing });

  assert.equal(result.signal, null);
  assert.notEqual(result.status, 0, 'workspace creation must fail under a missing TMPDIR');
  assert.doesNotMatch(result.stdout, /ok workspace is created directly beneath TMPDIR/);
  assert.throws(() => lstatSync(missing), { code: 'ENOENT' },
    'the missing TMPDIR must not be created on the caller\'s behalf');
  assert.throws(() => lstatSync(observed), { code: 'ENOENT' },
    'no workspace path may be recorded');
});
