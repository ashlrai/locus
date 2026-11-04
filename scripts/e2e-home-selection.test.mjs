// Run: node --test scripts/e2e-home-selection.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, lstatSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const script = join(root, 'scripts', 'e2e.sh');

// The unit under test is the script's own section 2: it selects the temporary
// root (${TMPDIR:-/tmp}), creates LOCUS_HOME there, installs the EXIT cleanup
// trap, and verifies the home sits directly beneath a supplied TMPDIR. The
// probes below run that block verbatim; stubs provide only log/ok/die and,
// where noted, a recording mktemp.
function homeSection() {
  const source = readFileSync(script, 'utf8');
  const begin = source.indexOf('export LOCUS_HOME\n');
  const endMarker = 'ok "isolated home is created directly beneath TMPDIR"\nfi';
  const end = source.indexOf(endMarker, begin);
  assert.ok(begin >= 0 && end > begin,
    'e2e.sh must contain the LOCUS_HOME temporary-root selection block');
  return source.slice(begin, end + endMarker.length);
}

const prelude = `set -euo pipefail
log() { :; }
ok() { printf 'ok %s\\n' "$*"; }
die() { printf 'FAIL %s\\n' "$*" >&2; exit 1; }
`;

function fixture(t) {
  const dir = mkdtempSync(join(tmpdir(), 'locus-e2e-home-selection-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  return dir;
}

// Runs the selection block. tmpdir === undefined leaves TMPDIR unset; extraEnv
// feeds the shim's record files. Returns the spawn result and the observed
// home path the probe writes before exit cleanup runs.
function select(t, { tmpdir, shim = '', tail = '', extraEnv = {} }) {
  const dir = fixture(t);
  const observed = join(dir, 'observed-home');
  const env = { ...process.env, OBSERVED_HOME: observed, ...extraEnv };
  if (tmpdir === undefined) delete env.TMPDIR;
  else env.TMPDIR = tmpdir;
  const result = spawnSync('bash', ['-c', `${prelude}${shim}
${homeSection()}
${tail}
printf '%s' "$LOCUS_HOME" > "$OBSERVED_HOME"
`], { env, encoding: 'utf8' });
  return { dir, observed, result };
}

test('LOCUS_HOME is created under the supplied TMPDIR', (t) => {
  const dir = fixture(t);
  const scratch = join(dir, 'custom tmp');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');

  const { observed, result } = select(t, { tmpdir: scratch, tail: `
[[ -d "$LOCUS_HOME" && -w "$LOCUS_HOME" && -x "$LOCUS_HOME" ]] || exit 71
mkdir -p "$LOCUS_HOME/bindings/nested dir" || exit 72
printf 'payload' > "$LOCUS_HOME/bindings/nested dir/f.toml" || exit 73
[[ "$(cat "$LOCUS_HOME/bindings/nested dir/f.toml")" == payload ]] || exit 74
` });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /ok isolated home ready/);
  assert.match(result.stdout, /ok isolated home is created directly beneath TMPDIR/,
    'the script must confirm the home landed beneath the supplied root');
  const home = readFileSync(observed, 'utf8');
  assert.equal(dirname(home), scratch, 'the home is created directly beneath TMPDIR');
  assert.match(home, /\/locus-e2e\.[A-Za-z0-9]{6}$/, 'the mktemp prefix and suffix shape hold');
  assert.throws(() => lstatSync(home), { code: 'ENOENT' },
    'the populated home must be removed on exit');
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
  assert.equal(readFileSync(template, 'utf8'), `${scratch}/locus-e2e.XXXXXX`,
    'the spaced root must arrive inside a single template argument');
  const home = readFileSync(observed, 'utf8');
  assert.equal(dirname(home), scratch);
  assert.match(home, /\/locus-e2e\.[^/\s]+$/, 'the home name is one unsplit component');
  assert.throws(() => lstatSync(home), { code: 'ENOENT' });
  assert.deepEqual(readdirSync(scratch), ['keep']);
});

for (const [label, tmpdir] of [['unset', undefined], ['empty', '']]) {
  test(`the selected root falls back to /tmp when TMPDIR is ${label}`, (t) => {
    const dir = fixture(t);
    const controlled = join(dir, 'controlled');
    const template = join(dir, 'template');
    mkdirSync(controlled);
    // Record the requested template and redirect creation into the controlled
    // directory so the probe never writes to the real /tmp.
    const shim = `mktemp() {
  if [[ "\${2:-}" == /tmp/locus-e2e.XXXXXX ]]; then
    printf '%s' "$2" > "$MKTEMP_TEMPLATE"
    command mktemp -d "$CONTROLLED/locus-e2e.XXXXXX"
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
    assert.equal(readFileSync(template, 'utf8'), '/tmp/locus-e2e.XXXXXX',
      `the script must select the /tmp root when TMPDIR is ${label}`);
    assert.doesNotMatch(result.stdout, /ok isolated home is created directly beneath TMPDIR/,
      'the supplied-TMPDIR confirmation must not print without a supplied TMPDIR');
    assert.equal(dirname(readFileSync(observed, 'utf8')), controlled);
    assert.deepEqual(readdirSync(controlled), [], 'the redirected home must be cleaned up');
  });
}

test('an inherited LOCUS_HOME does not change the selected root', (t) => {
  const dir = fixture(t);
  const scratch = join(dir, 'custom tmp');
  const inherited = join(dir, 'inherited home');
  mkdirSync(scratch);
  mkdirSync(inherited);
  writeFileSync(join(inherited, 'keep'), 'inherited');

  const { observed, result } = select(t, {
    tmpdir: scratch, extraEnv: { LOCUS_HOME: inherited },
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

test('a home created outside the supplied TMPDIR fails the selection check', (t) => {
  const dir = fixture(t);
  const scratch = join(dir, 'custom tmp');
  const wrongRoot = join(dir, 'wrong root');
  mkdirSync(scratch);
  mkdirSync(wrongRoot);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  // Answer with the expected name shape but relocate the home outside TMPDIR;
  // the containment check must stop the suite and the exit trap must still
  // remove the misplaced home.
  const shim = `mktemp() {
  if [[ "\${2:-}" == */locus-e2e.XXXXXX ]]; then
    command mktemp -d "$WRONG_ROOT/locus-e2e.XXXXXX"
    return
  fi
  command mktemp "$@"
}
`;

  const { result } = select(t, {
    tmpdir: scratch, shim, extraEnv: { WRONG_ROOT: wrongRoot },
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 1, 'the containment check must fail closed');
  assert.match(result.stdout, /ok isolated home ready/,
    'the cleanup trap is installed before the check runs');
  assert.doesNotMatch(result.stdout, /ok isolated home is created directly beneath TMPDIR/);
  assert.match(result.stderr, /FAIL LOCUS_HOME was not created directly beneath TMPDIR/);
  assert.deepEqual(readdirSync(wrongRoot), [], 'the misplaced home must be removed on exit');
  assert.deepEqual(readdirSync(scratch), ['keep']);
});
