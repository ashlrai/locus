// Run: node --test scripts/e2e-home-spaced-tmpdir.test.mjs
// The suite allocates LOCUS_HOME with `mktemp -d` beneath the caller-selected
// temporary root. When that root contains spaces the allocation must still
// succeed and the complete path must survive verbatim: one template argument
// to mktemp, one exported word for child processes, one argument to the exit
// trap's rm — never truncated at the first space, never split into siblings.
// The probes below run the script's own section 2 block; stubs provide only
// log/ok/die plus, where noted, a recording or relocating mktemp.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { lstatSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
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
  const dir = mkdtempSync(join(tmpdir(), 'locus-e2e-home-spaced-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  return dir;
}

// Runs the selection block beneath the supplied TMPDIR. extraEnv feeds the
// shim's record files. Returns the spawn result and the observed home path
// the probe writes before exit cleanup runs.
function select(t, { tmpdir, shim = '', tail = '', extraEnv = {} }) {
  const dir = fixture(t);
  const observed = join(dir, 'observed-home');
  const result = spawnSync('bash', ['-c', `${prelude}${shim}
${homeSection()}
${tail}
printf '%s' "$LOCUS_HOME" > "$OBSERVED_HOME"
`], {
    env: { ...process.env, TMPDIR: tmpdir, OBSERVED_HOME: observed, ...extraEnv },
    encoding: 'utf8',
  });
  return { dir, observed, result };
}

// Records mktemp's argument count, the template it was given, and the path it
// created, then lets the real command allocate the directory it was asked for.
const recordingMktemp = `mktemp() {
  printf '%s' "$#" > "$MKTEMP_ARGC"
  printf '%s' "$2" > "$MKTEMP_TEMPLATE"
  local created
  created="$(command mktemp "$@")" || return
  printf '%s' "$created" > "$MKTEMP_CREATED"
  printf '%s\\n' "$created"
}
`;

test('allocation beneath a space-containing TMPDIR preserves the complete LOCUS_HOME path', (t) => {
  const dir = fixture(t);
  const scratch = join(dir, 'tmp root with spaces');
  const argc = join(dir, 'mktemp-argc');
  const template = join(dir, 'mktemp-template');
  const created = join(dir, 'mktemp-created');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');

  const { observed, result } = select(t, {
    tmpdir: scratch,
    shim: recordingMktemp,
    tail: `
# The allocated home must be usable through the complete spaced path.
[[ -d "$LOCUS_HOME" && -w "$LOCUS_HOME" && -x "$LOCUS_HOME" ]] || exit 71
mkdir -p "$LOCUS_HOME/bindings/nested dir" || exit 72
printf 'payload' > "$LOCUS_HOME/bindings/nested dir/f.toml" || exit 73
[[ "$(cat "$LOCUS_HOME/bindings/nested dir/f.toml")" == payload ]] || exit 74
`,
    extraEnv: { MKTEMP_ARGC: argc, MKTEMP_TEMPLATE: template, MKTEMP_CREATED: created },
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, `allocation must succeed: ${result.stderr}`);
  assert.match(result.stdout, /ok isolated home ready/);
  assert.match(result.stdout, /ok isolated home is created directly beneath TMPDIR/,
    'the script must confirm the home landed beneath the spaced root');
  assert.equal(readFileSync(argc, 'utf8'), '2',
    'mktemp must receive exactly -d and one template');
  assert.equal(readFileSync(template, 'utf8'), `${scratch}/locus-e2e.XXXXXX`,
    'the spaced root must arrive inside a single template argument');
  const home = readFileSync(observed, 'utf8');
  // The complete path is preserved: the directory mktemp created, the path it
  // printed, and the path the script assigned are byte-for-byte identical.
  assert.equal(home, readFileSync(created, 'utf8'),
    'the assigned LOCUS_HOME must be the complete path mktemp allocated');
  assert.equal(dirname(home), scratch,
    'the home sits directly beneath the verbatim spaced root');
  assert.match(basename(home), /^locus-e2e\.[A-Za-z0-9]{6}$/,
    'the leaf is one unsplit mktemp component');
  assert.ok(home.includes(' '), 'the recorded path keeps its spaces');
  assert.throws(() => lstatSync(home), { code: 'ENOENT' },
    'the populated home must be removed on exit');
  assert.deepEqual(readdirSync(scratch), ['keep'],
    'cleanup must preserve unrelated files in the spaced root');
});

test('consecutive spaces and glob characters in TMPDIR survive verbatim', (t) => {
  const dir = fixture(t);
  const scratch = join(dir, 'tmp  dir [*] 2');
  const template = join(dir, 'mktemp-template');
  const created = join(dir, 'mktemp-created');
  const decoy = join(dir, 'tmp');
  mkdirSync(scratch);
  mkdirSync(decoy);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  writeFileSync(join(decoy, 'sentinel'), 'keep');
  const shim = `mktemp() {
  printf '%s' "$2" > "$MKTEMP_TEMPLATE"
  local created
  created="$(command mktemp "$@")" || return
  printf '%s' "$created" > "$MKTEMP_CREATED"
  printf '%s\\n' "$created"
}
`;

  const { observed, result } = select(t, {
    tmpdir: scratch, shim,
    extraEnv: { MKTEMP_TEMPLATE: template, MKTEMP_CREATED: created },
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(readFileSync(template, 'utf8'), `${scratch}/locus-e2e.XXXXXX`,
    'consecutive spaces and glob characters must reach mktemp unexpanded');
  const home = readFileSync(observed, 'utf8');
  assert.equal(home, readFileSync(created, 'utf8'));
  assert.equal(dirname(home), scratch,
    'the parent is the verbatim root — nothing collapsed, split or globbed');
  assert.match(basename(home), /^locus-e2e\.[A-Za-z0-9]{6}$/);
  assert.throws(() => lstatSync(home), { code: 'ENOENT' });
  assert.deepEqual(readdirSync(decoy), ['sentinel'],
    'no locus-e2e.* fragment may land at the first word-split parent');
  assert.deepEqual(readdirSync(scratch), ['keep']);
});

test('the exported LOCUS_HOME reaches a child process as the complete spaced path', (t) => {
  const dir = fixture(t);
  const scratch = join(dir, 'export tmp root');
  const childSeen = join(dir, 'child-seen');
  mkdirSync(scratch);

  const { observed, result } = select(t, {
    tmpdir: scratch,
    tail: `
# A child inherits the exported home and must see — and use — the same
# complete spaced path, never a prefix truncated at the first space.
bash -c 'printf %s "$LOCUS_HOME"' > "$CHILD_SEEN" || exit 71
bash -c 'mkdir -p "$LOCUS_HOME/child dir" && printf payload > "$LOCUS_HOME/child dir/f.toml"' \
  || exit 72
[[ "$(cat "$LOCUS_HOME/child dir/f.toml")" == payload ]] || exit 73
`,
    extraEnv: { CHILD_SEEN: childSeen },
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, result.stderr);
  const home = readFileSync(observed, 'utf8');
  assert.equal(readFileSync(childSeen, 'utf8'), home,
    'the child must observe the complete exported path');
  assert.equal(dirname(home), scratch);
  assert.throws(() => lstatSync(home), { code: 'ENOENT' });
});

test('the allocated home is private (0700) beneath a spaced TMPDIR', (t) => {
  const dir = fixture(t);
  const scratch = join(dir, 'mode tmp root');
  const mode = join(dir, 'home-mode');
  mkdirSync(scratch);
  const shim = `mktemp() {
  local created
  created="$(command mktemp "$@")" || return
  { stat -f %Lp "$created" 2>/dev/null || stat -c %a "$created"; } > "$HOME_MODE"
  printf '%s\\n' "$created"
}
`;

  const { result } = select(t, {
    tmpdir: scratch, shim, extraEnv: { HOME_MODE: mode },
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(readFileSync(mode, 'utf8').trim(), '700',
    'the home holds suite state and must be allocated owner-only');
});

// A mktemp regression that returns a path truncated at the first space must
// trip the containment check: the suite stops and the exit trap still removes
// the misplaced home before the failure reaches the caller.
test('a home allocated at a space-truncated path fails the selection check', (t) => {
  const dir = fixture(t);
  const scratch = join(dir, 'tmp dir root');
  const truncated = join(dir, 'tmp');
  const created = join(dir, 'truncated-created');
  mkdirSync(scratch);
  mkdirSync(truncated);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  const shim = `mktemp() {
  if [[ "\${2:-}" == */locus-e2e.XXXXXX ]]; then
    local d
    d="$(command mktemp -d "$TRUNCATED_ROOT/locus-e2e.XXXXXX")" || return
    printf '%s' "$d" > "$TRUNCATED_CREATED"
    printf '%s\\n' "$d"
    return
  fi
  command mktemp "$@"
}
`;

  const { result } = select(t, {
    tmpdir: scratch, shim,
    extraEnv: { TRUNCATED_ROOT: truncated, TRUNCATED_CREATED: created },
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 1, 'the containment check must fail closed');
  assert.match(result.stdout, /ok isolated home ready/,
    'the cleanup trap is installed before the check runs');
  assert.doesNotMatch(result.stdout, /ok isolated home is created directly beneath TMPDIR/);
  assert.match(result.stderr, /FAIL LOCUS_HOME was not created directly beneath TMPDIR/);
  const misplaced = readFileSync(created, 'utf8');
  assert.equal(dirname(misplaced), truncated,
    'the regression placed the home at the truncated root');
  assert.throws(() => lstatSync(misplaced), { code: 'ENOENT' },
    'the misplaced home must be removed by the exit trap');
  assert.deepEqual(readdirSync(truncated), []);
  assert.deepEqual(readdirSync(scratch), ['keep']);
});
