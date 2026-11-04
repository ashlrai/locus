// Run: node --test scripts/e2e-graph-home-selection.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { lstatSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const script = join(root, 'scripts', 'e2e.sh');

// The unit under test is the script's graph home lifecycle: the
// create_graph_home function selects the temporary root (${TMPDIR:-/tmp})
// and creates GRAPH_HOME there, and the e2e_cleanup EXIT trap removes it.
// The probes below run both verbatim; shims provide only a recording mktemp.
// The section-16 containment check on a relocated home is covered by
// scripts/e2e-graph-isolation.test.mjs. Two tests run the script's own
// section 2i rehearsal verbatim; the last group runs the unconditional
// repeated-creation block from the tail of section 16 verbatim beneath a
// controlled temporary root, plus a word-split decoy under the exit trap.
function graphParts() {
  const source = readFileSync(script, 'utf8');
  const cleanup = source.match(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m)?.[0];
  const createHome = source.match(/^create_graph_home\(\) \{[\s\S]*?^\}/m)?.[0];
  assert.ok(cleanup && createHome,
    'e2e.sh must contain the graph home lifecycle');
  return { cleanup, createHome };
}

function fixture(t) {
  const dir = mkdtempSync(join(tmpdir(), 'locus-e2e-graph-home-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  return dir;
}

// Runs create_graph_home under the real EXIT trap. tmpdir === undefined
// leaves TMPDIR unset; extraEnv feeds shim record files. Returns the spawn
// result and the home path the probe writes before exit cleanup runs.
function select(t, { tmpdir, shim = '', tail = '', extraEnv = {} }) {
  const dir = fixture(t);
  const observed = join(dir, 'observed-home');
  const mainHome = join(dir, 'main-home');
  mkdirSync(mainHome);
  const { cleanup, createHome } = graphParts();
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
create_graph_home
${tail}
printf '%s' "$GRAPH_HOME" > "$OBSERVED_HOME"
`], { env, encoding: 'utf8' });
  return { dir, observed, result };
}

test('GRAPH_HOME is created under the supplied TMPDIR', (t) => {
  const dir = fixture(t);
  const scratch = join(dir, 'custom tmp');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');

  const { observed, result } = select(t, { tmpdir: scratch, tail: `
[[ -d "$GRAPH_HOME" && -w "$GRAPH_HOME" && -x "$GRAPH_HOME" ]] || exit 71
mkdir -p "$GRAPH_HOME/bindings/nested dir" || exit 72
printf 'payload' > "$GRAPH_HOME/bindings/nested dir/f.toml" || exit 73
[[ "$(cat "$GRAPH_HOME/bindings/nested dir/f.toml")" == payload ]] || exit 74
` });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, result.stderr);
  const home = readFileSync(observed, 'utf8');
  assert.equal(dirname(home), scratch, 'the graph home is created directly beneath TMPDIR');
  assert.match(home, /\/locus-graph-imp\.[A-Za-z0-9]{6}$/, 'the mktemp prefix and suffix shape hold');
  assert.throws(() => lstatSync(home), { code: 'ENOENT' },
    'the populated graph home must be removed on exit');
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
  assert.equal(readFileSync(template, 'utf8'), `${scratch}/locus-graph-imp.XXXXXX`,
    'the spaced root must arrive inside a single template argument');
  const home = readFileSync(observed, 'utf8');
  assert.equal(dirname(home), scratch);
  assert.match(home, /\/locus-graph-imp\.[^/\s]+$/, 'the home name is one unsplit component');
  assert.throws(() => lstatSync(home), { code: 'ENOENT' });
  assert.deepEqual(readdirSync(scratch), ['keep']);
});

for (const [label, tmpdir] of [['unset', undefined], ['empty', '']]) {
  test(`the graph home root falls back to /tmp when TMPDIR is ${label}`, (t) => {
    const dir = fixture(t);
    const controlled = join(dir, 'controlled');
    const template = join(dir, 'template');
    mkdirSync(controlled);
    // Record the requested template and redirect creation into the controlled
    // directory so the probe never writes to the real /tmp.
    const shim = `mktemp() {
  if [[ "\${2:-}" == /tmp/locus-graph-imp.XXXXXX ]]; then
    printf '%s' "$2" > "$MKTEMP_TEMPLATE"
    command mktemp -d "$CONTROLLED/locus-graph-imp.XXXXXX"
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
    assert.equal(readFileSync(template, 'utf8'), '/tmp/locus-graph-imp.XXXXXX',
      `the script must select the /tmp root when TMPDIR is ${label}`);
    assert.equal(dirname(readFileSync(observed, 'utf8')), controlled);
    assert.deepEqual(readdirSync(controlled), [], 'the redirected home must be cleaned up');
  });
}

test('an inherited GRAPH_HOME does not change the selected root', (t) => {
  const dir = fixture(t);
  const scratch = join(dir, 'custom tmp');
  const inherited = join(dir, 'inherited graph home');
  mkdirSync(scratch);
  mkdirSync(inherited);
  writeFileSync(join(inherited, 'keep'), 'inherited');

  const { observed, result } = select(t, {
    tmpdir: scratch, extraEnv: { GRAPH_HOME: inherited },
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

// The suite rehearses two unconditional creations; each call must yield a
// distinct directory directly beneath the supplied TMPDIR.
test('repeated graph home creation yields distinct directories under TMPDIR', (t) => {
  const dir = fixture(t);
  const scratch = join(dir, 'custom tmp');
  const first = join(dir, 'first-home');
  const second = join(dir, 'second-home');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');

  const { result } = select(t, {
    tmpdir: scratch,
    extraEnv: { FIRST_HOME: first, SECOND_HOME: second },
    tail: `
graph_home_a="$GRAPH_HOME"
create_graph_home
graph_home_b="$GRAPH_HOME"
[[ "$graph_home_a" != "$graph_home_b" ]] || exit 75
GRAPH_HOME=""
printf '%s' "$graph_home_a" > "$FIRST_HOME"
printf '%s' "$graph_home_b" > "$SECOND_HOME"
rm -rf "$graph_home_a" "$graph_home_b"
`,
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, result.stderr);
  const homeA = readFileSync(first, 'utf8');
  const homeB = readFileSync(second, 'utf8');
  assert.notEqual(homeA, homeB, 'repeated creation must not reuse a directory');
  assert.equal(dirname(homeA), scratch);
  assert.equal(dirname(homeB), scratch);
  assert.match(homeA, /\/locus-graph-imp\.[^/\s]+$/);
  assert.match(homeB, /\/locus-graph-imp\.[^/\s]+$/);
  assert.throws(() => lstatSync(homeA), { code: 'ENOENT' });
  assert.throws(() => lstatSync(homeB), { code: 'ENOENT' });
  assert.deepEqual(readdirSync(scratch), ['keep']);
});

// The script's own section 2i rehearsal: a subshell exports a nested,
// doubled-space TMPDIR, creates the graph home beneath it, populates it the
// way the import fallback does (init with LOCUS_HOME="$GRAPH_HOME"), and
// relies on its EXIT trap. The probes below run the block verbatim; stubs
// provide log/ok/die, an init-capable locus binary and, where noted, a
// recording mktemp.
function section2i() {
  const source = readFileSync(script, 'utf8');
  const start = source.indexOf('log "2i.');
  const endMarker = 'ok "GRAPH_HOME creation succeeds beneath a TMPDIR containing spaces"';
  const end = source.indexOf(endMarker, start);
  assert.ok(start >= 0 && end > start, 'e2e.sh must contain the section 2i rehearsal');
  return source.slice(start, end + endMarker.length);
}

// A locus stub just real enough for the rehearsal: `init` populates the home
// it is pointed at, the way the fallback populates GRAPH_HOME before import.
function stubLocus(dir) {
  const bin = join(dir, 'bin');
  mkdirSync(bin);
  const locus = join(bin, 'locus');
  writeFileSync(locus, `#!/bin/sh
case "$1" in
  init) printf 'stub init\\n' > "$LOCUS_HOME/init-stamp" && exit 0 ;;
esac
exit 1
`, { mode: 0o755 });
  return locus;
}

// Runs the real section 2i (not a copy of its subshell) against a prepared
// main home. The block owns the subshell trap itself, so the main home and
// the nested temporary root stay inspectable after the spawn exits.
function rehearse2i(home, locus, { shim = '', extraEnv = {} } = {}) {
  const { cleanup, createHome } = graphParts();
  return spawnSync('bash', ['-c', `set -euo pipefail
${cleanup}
${shim}
${createHome}
log() { :; }
ok() { printf 'ok %s\\n' "$*"; }
die() { printf 'FAIL %s\\n' "$*" >&2; exit 1; }
LOCUS_HOME="$1"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
${section2i()}
`, 'graph-home-2i', home], {
    env: { ...process.env, LOCUS_BIN: locus, ...extraEnv }, encoding: 'utf8',
  });
}

test('section 2i creates the graph home beneath the space-containing TMPDIR', (t) => {
  const dir = fixture(t);
  const home = join(dir, 'main home');
  const outer = join(dir, 'outer tmp');
  const templates = join(dir, 'mktemp-templates');
  const argc = join(dir, 'mktemp-argc');
  mkdirSync(home);
  mkdirSync(outer);
  const locus = stubLocus(dir);
  // Record the requested templates and argument count, then let the real
  // command create the directory it was given. The outer TMPDIR stays visible
  // to prove the subshell-selected root is what reaches mktemp.
  const shim = `mktemp() {
  printf '%s\\n' "$2" >> "$MKTEMP_TEMPLATES"
  printf '%s' "$#" > "$MKTEMP_ARGC"
  command mktemp "$@"
}
`;

  const result = rehearse2i(home, locus, {
    shim,
    extraEnv: { TMPDIR: outer, MKTEMP_TEMPLATES: templates, MKTEMP_ARGC: argc },
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, `the 2i rehearsal must succeed: ${result.stderr}`);
  assert.match(result.stdout,
    /ok GRAPH_HOME creation succeeds beneath a TMPDIR containing spaces/);
  const selected = join(home, 'graph  home temp root');
  assert.equal(readFileSync(argc, 'utf8'), '2',
    'mktemp must receive exactly -d and one template');
  assert.deepEqual(readFileSync(templates, 'utf8').trim().split('\n'),
    [`${selected}/locus-graph-imp.XXXXXX`],
    'the subshell-selected root must reach mktemp inside one template argument');
  const recorded = readFileSync(join(selected, 'graph-home-path'), 'utf8');
  assert.equal(dirname(recorded), selected,
    'the rehearsal home is created directly beneath the supplied TMPDIR');
  assert.match(recorded, /\/locus-graph-imp\.[^/\s]+$/,
    'the home name is one unsplit component');
  assert.throws(() => lstatSync(recorded), { code: 'ENOENT' },
    'the populated rehearsal home must be removed on subshell exit');
  assert.deepEqual(readdirSync(selected), ['graph-home-path'],
    'only the recorded home path may remain beneath the supplied root');
  assert.deepEqual(readdirSync(outer), [],
    'the outer TMPDIR must not receive the graph home');
  assert.deepEqual(readdirSync(home), ['graph  home temp root'],
    'the subshell trap must leave the main home and the supplied root intact');
});

// A mktemp regression that relocates the rehearsal graph home outside the
// supplied root must stop the suite at the 2i subshell; the subshell trap
// still removes the misplaced directory before the failure reaches the caller.
test('a rehearsal graph home created outside the supplied TMPDIR stops section 2i', (t) => {
  const dir = fixture(t);
  const home = join(dir, 'main home');
  const wrongRoot = join(dir, 'wrong root');
  const created = join(dir, 'graph-home-created');
  mkdirSync(home);
  mkdirSync(wrongRoot);
  const locus = stubLocus(dir);
  const shim = `mktemp() {
  local d
  if [[ "\${2:-}" == */locus-graph-imp.XXXXXX ]]; then
    d="$(command mktemp -d "$WRONG_ROOT/locus-graph-imp.XXXXXX")"
    printf '%s' "$d" > "$HOME_CREATED"
    printf '%s\\n' "$d"
    return
  fi
  command mktemp "$@"
}
`;

  const result = rehearse2i(home, locus, {
    shim,
    extraEnv: { WRONG_ROOT: wrongRoot, HOME_CREATED: created },
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 1, 'the containment check must fail the suite');
  assert.match(result.stderr, /FAIL GRAPH_HOME was not created beneath the space-containing TMPDIR/);
  assert.doesNotMatch(result.stdout, /ok GRAPH_HOME creation succeeds/);
  const misplaced = readFileSync(created, 'utf8');
  assert.equal(dirname(misplaced), wrongRoot);
  assert.match(misplaced, /\/locus-graph-imp\.[^/]+$/);
  assert.throws(() => lstatSync(misplaced), { code: 'ENOENT' },
    'the misplaced graph home must be removed by the subshell exit trap');
  assert.deepEqual(readdirSync(wrongRoot), []);
  assert.deepEqual(readdirSync(join(home, 'graph  home temp root')), [],
    'the rehearsal exits before recording a home path');
});

// TMPDIR must name an existing directory: a missing one fails loudly instead
// of silently falling back to /tmp, and nothing is created in its place.
test('a nonexistent TMPDIR fails graph home creation without creating anything', (t) => {
  const dir = fixture(t);
  const missing = join(dir, 'does not exist');

  const { observed, result } = select(t, { tmpdir: missing });

  assert.equal(result.signal, null);
  assert.notEqual(result.status, 0, 'creation under a missing TMPDIR must fail');
  assert.throws(() => lstatSync(missing), { code: 'ENOENT' }, 'the missing root must not be created');
  assert.throws(() => lstatSync(observed), { code: 'ENOENT' }, 'no graph home may be recorded');
});

// macOS hands out TMPDIR with a trailing slash. The home must still land in
// that root, and the section-2i containment test (`${GRAPH_HOME%/*}` equals
// `$TMPDIR`) must keep holding for the unnormalized value.
test('a TMPDIR with a trailing slash still holds the graph home', (t) => {
  const dir = fixture(t);
  const scratch = join(dir, 'custom tmp');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');

  const { observed, result } = select(t, {
    tmpdir: `${scratch}/`,
    tail: '[[ "${GRAPH_HOME%/*}" == "$TMPDIR" && -d "$GRAPH_HOME" ]] || exit 76',
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, result.stderr);
  const home = readFileSync(observed, 'utf8');
  assert.ok(home.startsWith(`${scratch}//locus-graph-imp.`),
    'the supplied slash is kept verbatim and the home is a direct child of the root');
  assert.match(home, /\/locus-graph-imp\.[^/\s]+$/);
  assert.throws(() => lstatSync(home), { code: 'ENOENT' });
  assert.deepEqual(readdirSync(scratch), ['keep']);
});

// The security-home rehearsals carry a decoy at the first word-split fragment
// of a spaced TMPDIR (sections 2c2 and 2e); the graph home needs the same
// proof. An unquoted rm in the exit trap would split the spaced path and
// delete the sibling named by its first fragment, so the decoy must survive
// while the real graph home is still removed.
test('a word-split sibling of a spaced TMPDIR survives graph home cleanup', (t) => {
  const dir = fixture(t);
  const decoy = join(dir, 'graph');
  const scratch = join(dir, 'graph root with  spaces');
  mkdirSync(decoy);
  mkdirSync(scratch);
  writeFileSync(join(decoy, 'decoy-sentinel'), 'keep');

  const { observed, result } = select(t, { tmpdir: scratch });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, result.stderr);
  const home = readFileSync(observed, 'utf8');
  assert.equal(dirname(home), scratch);
  assert.throws(() => lstatSync(home), { code: 'ENOENT' },
    'the real graph home must still be removed');
  assert.equal(readFileSync(join(decoy, 'decoy-sentinel'), 'utf8'), 'keep',
    'cleanup must not reach the word-split sibling of the spaced root');
});

// The unconditional repeated-creation block at the tail of section 16 is
// extracted verbatim (not copied) so its own containment, distinctness and
// removal checks are the ones under test. Success beneath a space-containing
// TMPDIR is already covered through the full section-16 slice in
// scripts/e2e-graph-isolation.test.mjs; the probes below cover a plain
// controlled root, a trailing-slash root, and the block's two failure modes.
function repeatedBlock() {
  const source = readFileSync(script, 'utf8');
  const start = source.indexOf('create_graph_home; graph_home_a=');
  const endMarker = 'ok "two graph homes are distinct and live directly beneath the temp root"';
  const end = source.indexOf(endMarker, start);
  assert.ok(start >= 0 && end > start, 'e2e.sh must contain the repeated graph-home block');
  const block = source.slice(start, end + endMarker.length);
  assert.ok(block.includes('graph_root="${TMPDIR:-/tmp}"'),
    'the block must select the temporary root itself');
  return block;
}

// Runs the extracted block verbatim under the real EXIT trap, the same way
// section 16 reaches it after the graph import cleanup.
function repeatSection(t, { tmpdir, shim = '', extraEnv = {} }) {
  const dir = fixture(t);
  const mainHome = join(dir, 'main-home');
  mkdirSync(mainHome);
  const { cleanup, createHome } = graphParts();
  const env = { ...process.env, MAIN_HOME: mainHome, ...extraEnv };
  if (tmpdir === undefined) delete env.TMPDIR;
  else env.TMPDIR = tmpdir;
  const result = spawnSync('bash', ['-c', `set -euo pipefail
${cleanup}
${createHome}
${shim}
log() { :; }
ok() { printf 'ok %s\\n' "$*"; }
die() { printf 'FAIL %s\\n' "$*" >&2; exit 1; }
LOCUS_HOME="$MAIN_HOME"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
trap 'e2e_cleanup' EXIT
${repeatedBlock()}
`], { env, encoding: 'utf8' });
  return { dir, result };
}

test('the verbatim block keeps both repeated homes beneath a controlled TMPDIR', (t) => {
  const dir = fixture(t);
  const scratch = join(dir, 'controlled root');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');

  const { result } = repeatSection(t, { tmpdir: scratch });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout,
    /ok two graph homes are distinct and live directly beneath the temp root/);
  assert.deepEqual(readdirSync(scratch), ['keep'],
    'the block must remove both homes it created');
});

// A TMPDIR may end in a slash (macOS hands out such values); the block strips
// it from both the selected root and each parent before comparing, so a bare
// equality check would regress here.
test('the verbatim block honors a controlled TMPDIR with a trailing slash', (t) => {
  const dir = fixture(t);
  const scratch = join(dir, 'controlled root');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');

  const { result } = repeatSection(t, { tmpdir: `${scratch}/` });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /ok two graph homes are distinct/);
  assert.deepEqual(readdirSync(scratch), ['keep']);
});

// A mktemp regression that relocates the repeated homes must stop the suite at
// the block's own containment check. The misplaced pair is left beneath the
// wrong root (the script header documents leftover temp dirs on failure)
// while the supplied root stays untouched.
test('repeated homes created outside the controlled TMPDIR stop the block', (t) => {
  const dir = fixture(t);
  const scratch = join(dir, 'controlled root');
  const wrongRoot = join(dir, 'wrong root');
  mkdirSync(scratch);
  mkdirSync(wrongRoot);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  const shim = `mktemp() {
  if [[ "\${2:-}" == */locus-graph-imp.XXXXXX ]]; then
    command mktemp -d "$WRONG_ROOT/locus-graph-imp.XXXXXX"
    return
  fi
  command mktemp "$@"
}
`;

  const { result } = repeatSection(t, {
    tmpdir: scratch, shim, extraEnv: { WRONG_ROOT: wrongRoot },
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 1, 'the containment check must fail the suite');
  assert.match(result.stderr,
    /FAIL repeated graph homes were not created directly beneath the temp root/);
  assert.doesNotMatch(result.stdout, /ok two graph homes are distinct/);
  const misplaced = readdirSync(wrongRoot);
  assert.equal(misplaced.length, 2, 'both misplaced homes stay beneath the wrong root');
  assert.ok(misplaced.every((n) => n.startsWith('locus-graph-imp.')));
  assert.deepEqual(readdirSync(scratch), ['keep']);
});

// A regression that hands back one directory twice must die at the
// distinctness check before either home is removed.
test('a repeated creation returning the same directory stops the block', (t) => {
  const dir = fixture(t);
  const scratch = join(dir, 'controlled root');
  const seen = join(dir, 'seen-path');
  mkdirSync(scratch);
  const shim = `mktemp() {
  if [[ "\${2:-}" == */locus-graph-imp.XXXXXX ]]; then
    [[ -s "$SEEN_PATH" ]] || command mktemp -d "$2" > "$SEEN_PATH"
    cat "$SEEN_PATH"
    return
  fi
  command mktemp "$@"
}
`;

  const { result } = repeatSection(t, {
    tmpdir: scratch, shim, extraEnv: { SEEN_PATH: seen },
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 1, 'the distinctness check must fail the suite');
  assert.match(result.stderr,
    /FAIL repeated graph home creation returned the same directory/);
  const dup = readFileSync(seen, 'utf8').trim();
  assert.equal(dirname(dup), scratch);
  assert.deepEqual(readdirSync(scratch), [basename(dup)],
    'only the duplicated home is left beneath the supplied root');
});
