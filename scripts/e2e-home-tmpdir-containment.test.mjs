// Run: node --test scripts/e2e-home-tmpdir-containment.test.mjs
// Section 2 of e2e.sh selects the suite's temporary root before any fixture
// exists: a supplied, nonempty TMPDIR is the containment boundary the caller
// chose, and the allocated LOCUS_HOME must live inside it as a real directory
// entry. Selection must be evaluated at each allocation (the rehearsal
// subshells re-export TMPDIR to their own roots), must not drift to other
// temp variables or the working directory, and must fail closed — never fall
// back to another root — when the supplied TMPDIR does not exist. The probes
// below run the script's own selection block verbatim; stubs provide only
// log/ok/die plus, where noted, a recording mktemp.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, lstatSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const script = join(root, 'scripts', 'e2e.sh');

// The unit under test is the script's own section 2: it selects the temporary
// root (${TMPDIR:-/tmp}), creates LOCUS_HOME there, exports it, installs the
// EXIT cleanup trap, and verifies the home sits directly beneath a supplied
// TMPDIR. The probes below run that block verbatim.
function homeSection() {
  const source = readFileSync(script, 'utf8');
  const begin = source.indexOf('create_locus_home() {\n');
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
  const dir = mkdtempSync(join(tmpdir(), 'locus-e2e-home-containment-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  return dir;
}

// Runs the selection block beneath the supplied TMPDIR. extraEnv feeds the
// shim's record files and decoy roots; cwd, when given, is the probe's working
// directory. Returns the spawn result and the observed home path the probe
// writes before exit cleanup runs.
function select(t, { tmpdir, shim = '', tail = '', extraEnv = {}, cwd }) {
  const dir = fixture(t);
  const observed = join(dir, 'observed-home');
  const result = spawnSync('bash', ['-c', `${prelude}${shim}
${homeSection()}
${tail}
printf '%s' "$LOCUS_HOME" > "$OBSERVED_HOME"
`], {
    env: { ...process.env, TMPDIR: tmpdir, OBSERVED_HOME: observed, ...extraEnv },
    encoding: 'utf8',
    ...(cwd ? { cwd } : {}),
  });
  return { dir, observed, result };
}

test('a controlled TMPDIR contains the allocated directory', (t) => {
  const dir = fixture(t);
  const scratch = join(dir, 'controlled tmp root');
  const listing = join(dir, 'root-listing');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');

  const { observed, result } = select(t, {
    tmpdir: scratch,
    tail: `
# Containment must be physical, not just a matching prefix: the allocated
# home is a real directory entry inside the controlled root, not a link.
[[ -d "$LOCUS_HOME" && ! -L "$LOCUS_HOME" ]] || exit 71
[[ -d "$TMPDIR/\${LOCUS_HOME##*/}" ]] || exit 72
ls -A "$TMPDIR" | sort > "$ROOT_LISTING"
[[ "$(find "$TMPDIR" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')" == 1 ]] || exit 73
`,
    extraEnv: { ROOT_LISTING: listing },
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, `selection must succeed: ${result.stderr}`);
  assert.match(result.stdout, /ok isolated home is created directly beneath TMPDIR/,
    'the script must confirm the home landed beneath the controlled root');
  const home = readFileSync(observed, 'utf8');
  assert.equal(dirname(home), scratch);
  assert.match(basename(home), /^locus-e2e\.[^/]+$/,
    'the leaf is one mktemp component beneath the controlled root');
  assert.deepEqual(readFileSync(listing, 'utf8').trim().split('\n').sort(),
    [basename(home), 'keep'].sort(),
    'the controlled TMPDIR contains the allocated directory and nothing else');
  assert.throws(() => lstatSync(home), { code: 'ENOENT' },
    'the allocated home must be removed on exit');
  assert.deepEqual(readdirSync(scratch), ['keep'],
    'cleanup must preserve unrelated files in the controlled root');
});

test('selection ignores TMP, TEMP and the working directory', (t) => {
  const dir = fixture(t);
  const scratch = join(dir, 'controlled root');
  const tmpDecoy = join(dir, 'TMP decoy');
  const tempDecoy = join(dir, 'TEMP decoy');
  const cwdDecoy = join(dir, 'cwd decoy');
  mkdirSync(scratch);
  mkdirSync(tmpDecoy);
  mkdirSync(tempDecoy);
  mkdirSync(cwdDecoy);

  const { observed, result } = select(t, {
    tmpdir: scratch,
    cwd: cwdDecoy,
    extraEnv: { TMP: tmpDecoy, TEMP: tempDecoy },
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, `selection must succeed: ${result.stderr}`);
  const home = readFileSync(observed, 'utf8');
  assert.equal(dirname(home), scratch,
    'TMPDIR — not TMP, TEMP or the working directory — is the selected root');
  assert.throws(() => lstatSync(home), { code: 'ENOENT' });
  for (const decoy of [tmpDecoy, tempDecoy, cwdDecoy]) {
    assert.deepEqual(readdirSync(decoy), [],
      `${basename(decoy)} must not receive the allocated directory`);
  }
});

// Rehearsal subshells re-export TMPDIR and call create_locus_home again, so
// selection must be evaluated per call — never cached from the first
// allocation or bound at function-definition time.
test('each allocation selects the TMPDIR in effect at call time', (t) => {
  const dir = fixture(t);
  const firstRoot = join(dir, 'first root');
  const secondRoot = join(dir, 'second root');
  const observedHomes = join(dir, 'observed-homes');
  mkdirSync(firstRoot);
  mkdirSync(secondRoot);

  const { result } = select(t, {
    tmpdir: firstRoot,
    tail: `
first_home="$LOCUS_HOME"
export TMPDIR="$SECOND_ROOT"
create_locus_home
[[ "$first_home" != "$LOCUS_HOME" ]] || exit 71
[[ -d "$first_home" && "\${first_home%/*}" == "$FIRST_ROOT" ]] || exit 72
[[ -d "$LOCUS_HOME" && "\${LOCUS_HOME%/*}" == "$SECOND_ROOT" ]] || exit 73
rm -rf "$first_home"
printf '%s\\n%s' "$first_home" "$LOCUS_HOME" > "$OBSERVED_HOMES"
`,
    extraEnv: { FIRST_ROOT: firstRoot, SECOND_ROOT: secondRoot, OBSERVED_HOMES: observedHomes },
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, `re-selection must succeed: ${result.stderr}`);
  const [firstHome, secondHome] = readFileSync(observedHomes, 'utf8').trim().split('\n');
  assert.equal(dirname(firstHome), firstRoot,
    'the first allocation lands beneath the first root');
  assert.equal(dirname(secondHome), secondRoot,
    'the second allocation follows the re-exported TMPDIR');
  assert.throws(() => lstatSync(secondHome), { code: 'ENOENT' },
    'the exit trap removes the home selected last');
  assert.deepEqual(readdirSync(firstRoot), []);
  assert.deepEqual(readdirSync(secondRoot), []);
});

// A supplied TMPDIR is the boundary the caller chose: when it does not exist
// the suite must stop, not silently substitute /tmp or any other root. The
// recording shim proves the supplied template is the only request mktemp sees.
test('a supplied TMPDIR that does not exist fails closed without a /tmp fallback', (t) => {
  const dir = fixture(t);
  const missing = join(dir, 'missing root');
  const templates = join(dir, 'requested-templates');
  const shim = `mktemp() {
  printf '%s\\n' "$2" >> "$MKTEMP_TEMPLATES"
  command mktemp "$@"
}
`;

  const { observed, result } = select(t, {
    tmpdir: missing,
    shim,
    extraEnv: { MKTEMP_TEMPLATES: templates },
  });

  assert.equal(result.signal, null);
  assert.notEqual(result.status, 0, 'a missing controlled root must stop the suite');
  assert.doesNotMatch(result.stdout, /ok isolated home is created directly beneath TMPDIR/);
  assert.equal(readFileSync(templates, 'utf8').trim(), `${missing}/locus-e2e.XXXXXX`,
    'the script must request the supplied root only — never retry beneath /tmp');
  assert.equal(existsSync(observed), false, 'no home may be observed after the failure');
  assert.deepEqual(readdirSync(dir), ['requested-templates'],
    'no fallback directory may be allocated anywhere');
});
