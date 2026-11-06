// Run: node --test scripts/e2e-security-home-selection.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { lstatSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const script = join(root, 'scripts', 'e2e.sh');

// The unit under test is the script's security home lifecycle: the
// create_security_home function selects the temporary root (${TMPDIR:-/tmp})
// and creates SECURITY_HOME there, and the e2e_cleanup EXIT trap removes it.
// The probes below run both verbatim; shims provide only a recording mktemp.
function securityParts() {
  const source = readFileSync(script, 'utf8');
  const cleanup = source.match(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m)?.[0];
  const createHome = source.match(/^create_security_home\(\) \{[\s\S]*?^\}/m)?.[0];
  const createWs = source.match(/^create_security_workspace\(\) \{[\s\S]*?^\}/m)?.[0];
  assert.ok(cleanup && createHome && createWs,
    'e2e.sh must contain the security directory lifecycle');
  return { cleanup, createHome, createWs };
}

// The script's own section 2c rehearsal: a subshell exports a nested custom
// TMPDIR, creates both security directories beneath it, checks containment,
// and relies on its EXIT trap for cleanup.
function section2c() {
  const source = readFileSync(script, 'utf8');
  const start = source.indexOf('log "2c.');
  const endMarker = 'ok "distinct security directories honor nondefault TMPDIR and are cleaned on exit"';
  const end = source.indexOf(endMarker, start);
  assert.ok(start >= 0 && end > start, 'e2e.sh must contain the section 2c rehearsal');
  return source.slice(start, end + endMarker.length);
}

function fixture(t) {
  const dir = mkdtempSync(join(tmpdir(), 'locus-e2e-security-home-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  return dir;
}

// Runs create_security_home under the real EXIT trap. tmpdir === undefined
// leaves TMPDIR unset; extraEnv feeds shim record files. Returns the spawn
// result and the home path the probe writes before exit cleanup runs.
function select(t, { tmpdir, shim = '', tail = '', extraEnv = {} }) {
  const dir = fixture(t);
  const observed = join(dir, 'observed-home');
  const mainHome = join(dir, 'main-home');
  mkdirSync(mainHome);
  const { cleanup, createHome } = securityParts();
  const env = { ...process.env, MAIN_HOME: mainHome, OBSERVED_HOME: observed, ...extraEnv };
  if (tmpdir === undefined) delete env.TMPDIR;
  else env.TMPDIR = tmpdir;
  const result = spawnSync('bash', ['-c', `set -euo pipefail
${cleanup}
${createHome}
${shim}
LOCUS_HOME="$MAIN_HOME"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
trap 'e2e_cleanup' EXIT
create_security_home
${tail}
printf '%s' "$SECURITY_HOME" > "$OBSERVED_HOME"
`], { env, encoding: 'utf8' });
  return { dir, observed, result };
}

test('SECURITY_HOME is created under the supplied TMPDIR', (t) => {
  const dir = fixture(t);
  const scratch = join(dir, 'custom tmp');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');

  const { observed, result } = select(t, { tmpdir: scratch, tail: `
[[ -d "$SECURITY_HOME" && -w "$SECURITY_HOME" && -x "$SECURITY_HOME" ]] || exit 71
mkdir -p "$SECURITY_HOME/bindings/nested dir" || exit 72
printf 'payload' > "$SECURITY_HOME/bindings/nested dir/f.toml" || exit 73
[[ "$(cat "$SECURITY_HOME/bindings/nested dir/f.toml")" == payload ]] || exit 74
` });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, result.stderr);
  const home = readFileSync(observed, 'utf8');
  assert.equal(dirname(home), scratch, 'the security home is created directly beneath TMPDIR');
  assert.match(home, /\/locus-security-e2e\.[A-Za-z0-9]{6}$/, 'the mktemp prefix and suffix shape hold');
  assert.throws(() => lstatSync(home), { code: 'ENOENT' },
    'the populated security home must be removed on exit');
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
  assert.equal(readFileSync(template, 'utf8'), `${scratch}/locus-security-e2e.XXXXXX`,
    'the spaced root must arrive inside a single template argument');
  const home = readFileSync(observed, 'utf8');
  assert.equal(dirname(home), scratch);
  assert.match(home, /\/locus-security-e2e\.[^/\s]+$/, 'the home name is one unsplit component');
  assert.throws(() => lstatSync(home), { code: 'ENOENT' });
  assert.deepEqual(readdirSync(scratch), ['keep']);
});

for (const [label, tmpdir] of [['unset', undefined], ['empty', '']]) {
  test(`the security home root falls back to /tmp when TMPDIR is ${label}`, (t) => {
    const dir = fixture(t);
    const controlled = join(dir, 'controlled');
    const template = join(dir, 'template');
    mkdirSync(controlled);
    // Record the requested template and redirect creation into the controlled
    // directory so the probe never writes to the real /tmp.
    const shim = `mktemp() {
  if [[ "\${2:-}" == /tmp/locus-security-e2e.XXXXXX ]]; then
    printf '%s' "$2" > "$MKTEMP_TEMPLATE"
    command mktemp -d "$CONTROLLED/locus-security-e2e.XXXXXX"
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
    assert.equal(readFileSync(template, 'utf8'), '/tmp/locus-security-e2e.XXXXXX',
      `the script must select the /tmp root when TMPDIR is ${label}`);
    assert.equal(dirname(readFileSync(observed, 'utf8')), controlled);
    assert.deepEqual(readdirSync(controlled), [], 'the redirected home must be cleaned up');
  });
}

test('an inherited SECURITY_HOME does not change the selected root', (t) => {
  const dir = fixture(t);
  const scratch = join(dir, 'custom tmp');
  const inherited = join(dir, 'inherited security home');
  mkdirSync(scratch);
  mkdirSync(inherited);
  writeFileSync(join(inherited, 'keep'), 'inherited');

  const { observed, result } = select(t, {
    tmpdir: scratch, extraEnv: { SECURITY_HOME: inherited },
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, result.stderr);
  const home = readFileSync(observed, 'utf8');
  assert.equal(dirname(home), scratch,
    'the selected root is the supplied TMPDIR, not the inherited home');
  assert.notEqual(home, inherited);
  assert.throws(() => lstatSync(home), { code: 'ENOENT' });
  assert.equal(readFileSync(join(inherited, 'keep'), 'utf8'), 'inherited',
    'the inherited home must survive untouched');
});

// Runs the real section 2c (not a copy of its subshell) with a recording
// mktemp: the custom TMPDIR the subshell exports must reach both security
// templates, and the subshell trap must remove the created home.
test('section 2c passes the custom TMPDIR to security home creation', (t) => {
  const dir = fixture(t);
  const mainHome = join(dir, 'main home');
  const templates = join(dir, 'templates');
  mkdirSync(mainHome);
  const { cleanup, createHome, createWs } = securityParts();
  const result = spawnSync('bash', ['-c', `set -euo pipefail
${cleanup}
mktemp() { printf '%s\\n' "$2" >> "$TEST_TEMPLATES"; command mktemp "$@"; }
${createHome}
${createWs}
log() { :; }
ok() { printf 'ok %s\\n' "$*"; }
die() { printf 'FAIL %s\\n' "$*" >&2; exit 1; }
LOCUS_HOME="$1"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
${section2c()}
`, 'security-2c', mainHome], {
    env: { ...process.env, TEST_TEMPLATES: templates },
    encoding: 'utf8',
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, `the 2c rehearsal must succeed: ${result.stderr}`);
  assert.match(result.stdout, /ok distinct security directories honor nondefault TMPDIR and are cleaned on exit/);
  const securityTmp = join(mainHome, 'custom temp');
  const calls = readFileSync(templates, 'utf8').trim().split('\n');
  assert.deepEqual(calls, [
    `${securityTmp}/locus-security-e2e.XXXXXX`,
    `${securityTmp}/locus-security-ws.XXXXXX`,
  ], 'both security directories must be requested beneath the custom TMPDIR');
  const home = readFileSync(join(securityTmp, 'security-home-path'), 'utf8');
  assert.equal(dirname(home), securityTmp, 'the security home sits directly beneath the custom TMPDIR');
  assert.match(home, /\/locus-security-e2e\.[^/\s]+$/, 'the home name is one unsplit component');
  assert.throws(() => lstatSync(home), { code: 'ENOENT' },
    'the rehearsal home must be removed on subshell exit');
});

// A mktemp regression that relocates the security home must stop the suite at
// the 2c subshell; the subshell trap still removes the misplaced directory.
test('a security home created outside the custom TMPDIR stops section 2c', (t) => {
  const dir = fixture(t);
  const mainHome = join(dir, 'main home');
  const wrongRoot = join(dir, 'wrong root');
  const created = join(dir, 'security-home-created');
  mkdirSync(mainHome);
  mkdirSync(wrongRoot);
  const { cleanup, createHome, createWs } = securityParts();
  const result = spawnSync('bash', ['-c', `set -euo pipefail
${cleanup}
mktemp() {
  local d
  if [[ "\${2:-}" == */locus-security-e2e.XXXXXX ]]; then
    d="$(command mktemp -d "$WRONG_ROOT/locus-security-e2e.XXXXXX")"
    printf '%s' "$d" > "$HOME_CREATED"
    printf '%s\\n' "$d"
    return
  fi
  command mktemp "$@"
}
${createHome}
${createWs}
log() { :; }
ok() { printf 'ok %s\\n' "$*"; }
die() { printf 'FAIL %s\\n' "$*" >&2; exit 1; }
LOCUS_HOME="$1"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
${section2c()}
`, 'security-2c-misplaced', mainHome], {
    env: { ...process.env, WRONG_ROOT: wrongRoot, HOME_CREATED: created },
    encoding: 'utf8',
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 1, 'the 2c containment check must fail the suite');
  assert.match(result.stderr, /FAIL security directories were not created distinctly beneath the supplied TMPDIR/);
  assert.doesNotMatch(result.stdout, /ok distinct security directories honor nondefault TMPDIR/);
  const home = readFileSync(created, 'utf8');
  assert.equal(dirname(home), wrongRoot);
  assert.match(home, /\/locus-security-e2e\.[^/]+$/);
  assert.throws(() => lstatSync(home), { code: 'ENOENT' },
    'the misplaced security home must be removed by the subshell exit trap');
  assert.deepEqual(readdirSync(wrongRoot), []);
});

// The section 24 selection block: both fixtures are created beneath the
// caller's TMPDIR, then a containment check stops the suite before the probes
// populate them. A mktemp regression that relocates the security home must
// trip that check; the exit trap still removes the misplaced directory.
function section24Selection() {
  const source = readFileSync(script, 'utf8');
  const start = source.indexOf('log "24.');
  const endMarker = 'ok "security directories are created directly beneath TMPDIR"\nfi';
  const end = source.indexOf(endMarker, start);
  assert.ok(start >= 0 && end > start, 'e2e.sh must contain the section 24 security selection block');
  return source.slice(start, end + endMarker.length);
}

test('a security home created outside the controlled TMPDIR stops section 24', (t) => {
  const dir = fixture(t);
  const scratch = join(dir, 'controlled tmp');
  const wrongRoot = join(dir, 'wrong root');
  const mainHome = join(dir, 'main home');
  const created = join(dir, 'home-created');
  mkdirSync(scratch);
  mkdirSync(wrongRoot);
  mkdirSync(mainHome);
  const { cleanup, createHome, createWs } = securityParts();
  const result = spawnSync('bash', ['-c', `set -euo pipefail
${cleanup}
mktemp() {
  if [[ "\${2:-}" == */locus-security-e2e.XXXXXX ]]; then
    d="$(command mktemp -d "$WRONG_ROOT/locus-security-e2e.XXXXXX")"
    printf '%s' "$d" > "$HOME_CREATED"
    printf '%s\\n' "$d"
    return
  fi
  command mktemp "$@"
}
${createHome}
${createWs}
log() { :; }
ok() { printf 'ok %s\\n' "$*"; }
die() { printf 'FAIL %s\\n' "$*" >&2; exit 1; }
LOCUS_HOME="$TEST_HOME"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
trap 'e2e_cleanup' EXIT
${section24Selection()}
`, 'security-24-misplaced-home', mainHome], {
    env: { ...process.env, TMPDIR: scratch, TEST_HOME: mainHome, WRONG_ROOT: wrongRoot, HOME_CREATED: created },
    encoding: 'utf8',
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 1, 'the containment check must fail the suite');
  assert.match(result.stderr, /FAIL security directories were not created directly beneath TMPDIR/);
  assert.doesNotMatch(result.stdout, /ok security directories are created directly beneath TMPDIR/);
  const home = readFileSync(created, 'utf8');
  assert.equal(dirname(home), wrongRoot);
  assert.match(home, /\/locus-security-e2e\.[^/]+$/);
  assert.throws(() => lstatSync(home), { code: 'ENOENT' },
    'the misplaced security home must be removed by the exit trap');
  assert.deepEqual(readdirSync(wrongRoot), []);
  assert.deepEqual(readdirSync(scratch), [], 'the correctly placed workspace must be removed too');
});

// The script's own section 2c2 rehearsal: a subshell exports a nested,
// doubled-space TMPDIR, creates only the security home beneath it, exercises
// the nested store path the probes use, and relies on its EXIT trap.
function section2c2() {
  const source = readFileSync(script, 'utf8');
  const start = source.indexOf('log "2c2.');
  const endMarker = 'ok "SECURITY_HOME is created and removed beneath a TMPDIR containing spaces"';
  const end = source.indexOf(endMarker, start);
  assert.ok(start >= 0 && end > start, 'e2e.sh must contain the section 2c2 rehearsal');
  return source.slice(start, end + endMarker.length);
}

// Runs the real section 2c2 (not a copy of its subshell) against a prepared
// main home. The block owns the subshell trap itself, so the main home and
// the nested temporary root stay inspectable after the spawn exits.
function rehearse2c2(home, { shim = '', extraEnv = {} } = {}) {
  const { cleanup, createHome } = securityParts();
  return spawnSync('bash', ['-c', `set -euo pipefail
${cleanup}
${shim}
${createHome}
log() { :; }
ok() { printf 'ok %s\\n' "$*"; }
die() { printf 'FAIL %s\\n' "$*" >&2; exit 1; }
LOCUS_HOME="$1"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
${section2c2()}
`, 'security-home-2c2', home], { env: { ...process.env, ...extraEnv }, encoding: 'utf8' });
}

test('section 2c2 creates the security home beneath the space-containing TMPDIR', (t) => {
  const dir = fixture(t);
  const home = join(dir, 'main home');
  const outer = join(dir, 'outer tmp');
  const templates = join(dir, 'mktemp-templates');
  const argc = join(dir, 'mktemp-argc');
  mkdirSync(home);
  mkdirSync(outer);
  // Record the requested templates and argument count, then let the real
  // command create the directory it was given. The outer TMPDIR stays visible
  // to prove the subshell-selected root is what reaches mktemp.
  const shim = `mktemp() {
  printf '%s\\n' "$2" >> "$MKTEMP_TEMPLATES"
  printf '%s' "$#" > "$MKTEMP_ARGC"
  command mktemp "$@"
}
`;

  const result = rehearse2c2(home, {
    shim,
    extraEnv: { TMPDIR: outer, MKTEMP_TEMPLATES: templates, MKTEMP_ARGC: argc },
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, `the 2c2 rehearsal must succeed: ${result.stderr}`);
  assert.match(result.stdout,
    /ok SECURITY_HOME is created and removed beneath a TMPDIR containing spaces/);
  const selected = join(home, 'sec tmp  root');
  assert.equal(readFileSync(argc, 'utf8'), '2',
    'mktemp must receive exactly -d and one template');
  assert.deepEqual(readFileSync(templates, 'utf8').trim().split('\n'),
    [`${selected}/locus-security-e2e.XXXXXX`],
    'the subshell-selected root must reach mktemp inside one template argument');
  const recorded = readFileSync(join(selected, 'security-home-path'), 'utf8');
  assert.equal(dirname(recorded), selected,
    'the rehearsal home is created directly beneath the exact supplied path');
  assert.match(recorded, /\/locus-security-e2e\.[^/\s]+$/,
    'the home name is one unsplit component');
  assert.throws(() => lstatSync(recorded), { code: 'ENOENT' },
    'the populated rehearsal home must be removed on subshell exit');
  assert.deepEqual(readdirSync(selected), ['security-home-path'],
    'only the recorded home path may remain beneath the supplied root');
  assert.deepEqual(readdirSync(outer), [],
    'the outer TMPDIR must not receive the security home');
  assert.deepEqual(readdirSync(home), ['sec', 'sec tmp  root'],
    'the subshell trap must leave the main home and the supplied root intact');
});

// A mktemp regression that relocates the rehearsal security home outside the
// selected root must stop the suite at the 2h subshell; the subshell trap
// still removes the misplaced directory before the failure reaches the caller.
test('a rehearsal security home created outside the supplied TMPDIR stops section 2c2', (t) => {
  const dir = fixture(t);
  const home = join(dir, 'main home');
  const wrongRoot = join(dir, 'wrong root');
  const created = join(dir, 'security-home-created');
  mkdirSync(home);
  mkdirSync(wrongRoot);
  const shim = `mktemp() {
  local d
  if [[ "\${2:-}" == */locus-security-e2e.XXXXXX ]]; then
    d="$(command mktemp -d "$WRONG_ROOT/locus-security-e2e.XXXXXX")"
    printf '%s' "$d" > "$HOME_CREATED"
    printf '%s\\n' "$d"
    return
  fi
  command mktemp "$@"
}
`;

  const result = rehearse2c2(home, {
    shim,
    extraEnv: { WRONG_ROOT: wrongRoot, HOME_CREATED: created },
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 1, 'the containment check must fail the suite');
  assert.match(result.stderr, /FAIL SECURITY_HOME was not created beneath the space-containing TMPDIR/);
  assert.doesNotMatch(result.stdout, /ok SECURITY_HOME is created and removed beneath a TMPDIR containing spaces/);
  const misplaced = readFileSync(created, 'utf8');
  assert.equal(dirname(misplaced), wrongRoot);
  assert.match(misplaced, /\/locus-security-e2e\.[^/]+$/);
  assert.throws(() => lstatSync(misplaced), { code: 'ENOENT' },
    'the misplaced security home must be removed by the subshell exit trap');
  assert.deepEqual(readdirSync(wrongRoot), []);
  assert.deepEqual(readdirSync(join(home, 'sec tmp  root')), [],
    'the rehearsal exits before recording a home path');
});

// Runs the real section 24 fixture block under the real EXIT trap.
// tmpdir === undefined leaves TMPDIR unset; extraEnv feeds shim record files.
// Returns the spawn result and the files the probe writes the created paths
// to before exit cleanup runs.
function select24(t, { tmpdir, shim = '', tail = '', extraEnv = {} }) {
  const dir = fixture(t);
  const observed = join(dir, 'observed-home');
  const observedWs = join(dir, 'observed-ws');
  const mainHome = join(dir, 'main-home');
  mkdirSync(mainHome);
  const { cleanup, createHome, createWs } = securityParts();
  const env = { ...process.env, MAIN_HOME: mainHome, OBSERVED_HOME: observed,
    OBSERVED_WS: observedWs, ...extraEnv };
  if (tmpdir === undefined) delete env.TMPDIR;
  else env.TMPDIR = tmpdir;
  const result = spawnSync('bash', ['-c', `set -euo pipefail
${cleanup}
${createHome}
${createWs}
${shim}
log() { :; }
ok() { printf 'ok %s\\n' "$*"; }
die() { printf 'FAIL %s\\n' "$*" >&2; exit 1; }
LOCUS_HOME="$MAIN_HOME"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
trap 'e2e_cleanup' EXIT
${section24Selection()}
${tail}
printf '%s' "$SECURITY_HOME" > "$OBSERVED_HOME"
printf '%s' "$SECURITY_WS" > "$OBSERVED_WS"
`], { env, encoding: 'utf8' });
  return { dir, observed, observedWs, result };
}


// Runs the real section 24 block (not a copy of its mktemp calls) with a
// recording mktemp: the supplied TMPDIR must reach both security templates,
// both fixtures must land directly beneath it, and the exit trap removes them.
test('section 24 creates both security directories beneath the supplied TMPDIR', (t) => {
  const dir = fixture(t);
  const scratch = join(dir, 'custom tmp');
  const templates = join(dir, 'templates');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  const shim = `mktemp() { printf '%s\\n' "$2" >> "$TEST_TEMPLATES"; command mktemp "$@"; }
`;

  const { observed, observedWs, result } = select24(t, {
    tmpdir: scratch, shim,
    tail: `
[[ -d "$SECURITY_HOME" && -w "$SECURITY_HOME" && -d "$SECURITY_WS" && \\
  ! "$SECURITY_HOME" -ef "$SECURITY_WS" ]] || exit 71
touch "$SECURITY_HOME/cleanup-sentinel" "$SECURITY_WS/cleanup-sentinel" || exit 72
`,
    extraEnv: { TEST_TEMPLATES: templates },
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /ok security directories are created directly beneath TMPDIR/);
  assert.deepEqual(readFileSync(templates, 'utf8').trim().split('\n'), [
    `${scratch}/locus-security-e2e.XXXXXX`,
    `${scratch}/locus-security-ws.XXXXXX`,
  ], 'both security fixtures must be requested beneath the supplied TMPDIR');
  const home = readFileSync(observed, 'utf8');
  const workspace = readFileSync(observedWs, 'utf8');
  assert.equal(dirname(home), scratch, 'the security home sits directly beneath TMPDIR');
  assert.equal(dirname(workspace), scratch, 'the security workspace sits directly beneath TMPDIR');
  assert.match(home, /\/locus-security-e2e\.[A-Za-z0-9]{6}$/, 'the mktemp prefix and suffix shape hold');
  assert.match(workspace, /\/locus-security-ws\.[A-Za-z0-9]{6}$/);
  assert.throws(() => lstatSync(home), { code: 'ENOENT' },
    'the populated security home must be removed on exit');
  assert.throws(() => lstatSync(workspace), { code: 'ENOENT' },
    'the populated security workspace must be removed on exit');
  assert.deepEqual(readdirSync(scratch), ['keep'], 'cleanup must preserve unrelated files');
});

// A mktemp regression that relocates either security fixture must stop the
// suite at the section 24 containment check, before the probes run; the exit
// trap still removes the misplaced directory.
for (const [label, prefix, record] of [
  ['security home', 'locus-security-e2e', 'security-home-created'],
  ['security workspace', 'locus-security-ws', 'security-ws-created'],
]) {
  test(`a ${label} created outside the supplied TMPDIR stops section 24`, (t) => {
    const dir = fixture(t);
    const scratch = join(dir, 'custom tmp');
    const wrongRoot = join(dir, 'wrong root');
    const created = join(dir, record);
    mkdirSync(scratch);
    mkdirSync(wrongRoot);
    writeFileSync(join(scratch, 'keep'), 'unrelated');
    // Answer with the expected name shape but relocate the fixture outside
    // TMPDIR; the containment check must fail closed.
    const shim = `mktemp() {
  local d
  if [[ "\${2:-}" == */${prefix}.XXXXXX ]]; then
    d="$(command mktemp -d "$WRONG_ROOT/${prefix}.XXXXXX")"
    printf '%s' "$d" > "$FIXTURE_CREATED"
    printf '%s\\n' "$d"
    return
  fi
  command mktemp "$@"
}
`;

    const { result } = select24(t, {
      tmpdir: scratch, shim,
      extraEnv: { WRONG_ROOT: wrongRoot, FIXTURE_CREATED: created },
    });

    assert.equal(result.signal, null);
    assert.equal(result.status, 1, 'the section 24 containment check must fail the suite');
    assert.match(result.stderr, /FAIL security directories were not created directly beneath TMPDIR/);
    assert.doesNotMatch(result.stdout, /ok security directories are created directly beneath TMPDIR/);
    const fixturePath = readFileSync(created, 'utf8');
    assert.equal(dirname(fixturePath), wrongRoot);
    assert.match(fixturePath, new RegExp(`/${prefix}\\.[^/]+$`));
    assert.throws(() => lstatSync(fixturePath), { code: 'ENOENT' },
      `the misplaced ${label} must be removed by the exit trap`);
    assert.deepEqual(readdirSync(wrongRoot), []);
    assert.deepEqual(readdirSync(scratch), ['keep']);
  });
}

// With no supplied TMPDIR there is no caller-chosen boundary to check; the
// block must still create both fixtures under the /tmp fallback and clean them.
for (const [label, tmpdir] of [['unset', undefined], ['empty', '']]) {
  test(`section 24 selects the /tmp root and skips the check when TMPDIR is ${label}`, (t) => {
    const dir = fixture(t);
    const controlled = join(dir, 'controlled');
    const templates = join(dir, 'templates');
    mkdirSync(controlled);
    // Record the requested templates and redirect creation into the controlled
    // directory so the probe never writes to the real /tmp.
    const shim = `mktemp() {
  printf '%s\\n' "$2" >> "$TEST_TEMPLATES"
  case "\${2:-}" in
    /tmp/locus-security-e2e.XXXXXX) command mktemp -d "$CONTROLLED/locus-security-e2e.XXXXXX" ;;
    /tmp/locus-security-ws.XXXXXX) command mktemp -d "$CONTROLLED/locus-security-ws.XXXXXX" ;;
    *) command mktemp "$@" ;;
  esac
}
`;

    const { observed, observedWs, result } = select24(t, {
      tmpdir, shim,
      extraEnv: { CONTROLLED: controlled, TEST_TEMPLATES: templates },
    });

    assert.equal(result.signal, null);
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(readFileSync(templates, 'utf8').trim().split('\n'), [
      '/tmp/locus-security-e2e.XXXXXX',
      '/tmp/locus-security-ws.XXXXXX',
    ], `the script must select the /tmp root when TMPDIR is ${label}`);
    assert.doesNotMatch(result.stdout, /ok security directories are created directly beneath TMPDIR/,
      'the supplied-TMPDIR confirmation must not print without a supplied TMPDIR');
    for (const record of [observed, observedWs]) {
      const created = readFileSync(record, 'utf8');
      assert.equal(dirname(created), controlled);
      assert.throws(() => lstatSync(created), { code: 'ENOENT' },
        'the redirected fixture must be cleaned up');
    }
    assert.deepEqual(readdirSync(controlled), []);
  });
}
