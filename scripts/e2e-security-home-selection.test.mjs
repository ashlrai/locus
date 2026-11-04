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
