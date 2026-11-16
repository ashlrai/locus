// Run: node --test scripts/e2e-workspace-spaced-tmpdir.test.mjs
// WS_DIR is the working directory the allowlist probes cd into. With a
// space-containing TMPDIR the allocation must land directly beneath that root
// as one unsplit path, stay usable (nested writes, the .locus.toml allowlist,
// entry), be private to the owner, and be removed by the exit trap — without
// touching the supplied root or a sibling at the root's first word-split
// fragment. These tests run the script's own create_workspace and
// e2e_cleanup verbatim; a doctored cleanup proves the guard distinguishes an
// unquoted-rm regression.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, lstatSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'e2e.sh');

function lifecycle() {
  const source = readFileSync(script, 'utf8');
  const cleanup = source.match(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m)?.[0];
  const create = source.match(/^create_workspace\(\) \{[\s\S]*?^\}/m)?.[0];
  assert.ok(cleanup && create, 'e2e.sh must define the workspace lifecycle');
  return { cleanup, create };
}

// The temporary root starts with the word "workspace" followed by a doubled
// space, so a sibling named "workspace" is exactly where an unquoted expansion
// of the workspace path would strike first.
function fixture(t) {
  const root = mkdtempSync(join(tmpdir(), 'locus-ws-space-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const decoy = join(root, 'workspace');
  const scratch = join(root, 'workspace  ws tmp root');
  const observed = join(root, 'workspace-path');
  const mode = join(root, 'mode');
  const argc = join(root, 'mktemp-argc');
  const args = join(root, 'mktemp-args');
  mkdirSync(decoy);
  mkdirSync(scratch);
  writeFileSync(join(decoy, 'decoy-sentinel'), 'decoy');
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  return { root, decoy, scratch, observed, mode, argc, args };
}

// The complete spaced root must reach mktemp inside ONE template argument.
// The recorder counts and saves each call's arguments before delegating: an
// unquoted expansion would word-split "workspace  ws tmp root" into four
// arguments — an outcome check alone cannot tell that apart from a correct
// call that happened to create the directory.
const mktempRecorder = `mktemp() {
  printf '%s\\n' "$#" > "$MKTEMP_ARGC"
  printf '%s\\n' "$@" > "$MKTEMP_ARGS"
  command mktemp "$@"
}
`;

function probe(f, body, { buggy = false, shim = '' } = {}) {
  const { cleanup, create } = lifecycle();
  // With `buggy`, the later definition wins: the quoting regression the guard
  // must catch. Every other variable stays quoted so only WS_DIR splits.
  const override = buggy
    ? 'e2e_cleanup() { rm -rf "$LOCUS_HOME" ${WS_DIR:-} "${GRAPH_HOME:-}" "${SECURITY_HOME:-}" "${SECURITY_WS:-}"; }'
    : '';
  return spawnSync('bash', ['-c', `set -euo pipefail
${cleanup}
${create}
${override}
${shim}
TMPDIR="$1"
LOCUS_HOME="$TMPDIR/main-home"
mkdir "$LOCUS_HOME"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
trap 'e2e_cleanup' EXIT
create_workspace
${body}
`, 'workspace-spaced-tmpdir', f.scratch], {
    cwd: f.root,
    env: {
      ...process.env,
      TMPDIR: f.scratch,
      OBSERVED: f.observed,
      MODE_RECORD: f.mode,
      MKTEMP_ARGC: f.argc,
      MKTEMP_ARGS: f.args,
    },
    encoding: 'utf8',
  });
}

test('the allocated workspace is usable and removed on exit beneath a spaced TMPDIR', (t) => {
  const f = fixture(t);
  const result = probe(f, `[[ -d "$WS_DIR" && "\${WS_DIR%/*}" == "$TMPDIR" && \\
  "\${WS_DIR##*/}" == locus-ws.* ]] || exit 71
{ stat -f %Lp "$WS_DIR" 2>/dev/null || stat -c %a "$WS_DIR"; } > "$MODE_RECORD"
# Usable, not just present: nested spaced paths round-trip, the allowlist
# lands inside it (as the real probes require), and entering it resolves to
# one physical path beneath the spaced root.
mkdir -p "$WS_DIR/probe dir" || exit 72
printf 'payload' > "$WS_DIR/probe dir/f.toml" || exit 73
[[ "$(cat "$WS_DIR/probe dir/f.toml")" == payload ]] || exit 74
printf 'default = "acme"\\n' > "$WS_DIR/.locus.toml" || exit 75
[[ -s "$WS_DIR/.locus.toml" ]] || exit 76
[[ "$(cd "$WS_DIR" && pwd -P)" == "$(cd "$TMPDIR" && pwd -P)/\${WS_DIR##*/}" ]] || exit 77
touch "$WS_DIR/cleanup-sentinel"
printf '%s' "$WS_DIR" > "$OBSERVED"
`);

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, `probe failed: ${result.stderr}`);
  const workspace = readFileSync(f.observed, 'utf8');
  assert.equal(dirname(workspace), f.scratch, 'allocated directly beneath the spaced TMPDIR');
  assert.match(workspace, /\/locus-ws\.[^/\s]+$/, 'the workspace name is one unsplit component');
  assert.ok(workspace.includes(' '), 'the allocated path must contain spaces for this test to mean anything');
  assert.equal(readFileSync(f.mode, 'utf8').trim(), '700', 'mktemp -d must allocate an owner-only directory');
  assert.throws(() => lstatSync(workspace), { code: 'ENOENT' }, 'the workspace must be removed on exit');
  assert.deepEqual(readdirSync(f.scratch).sort(), ['keep'], 'the supplied TMPDIR keeps only its own entries');
  assert.equal(readFileSync(join(f.decoy, 'decoy-sentinel'), 'utf8'), 'decoy',
    'cleanup must leave the word-split sibling of the spaced TMPDIR untouched');
});

test('the complete spaced path reaches mktemp as one template argument', (t) => {
  const f = fixture(t);
  const result = probe(f, `[[ -d "$WS_DIR" && "\${WS_DIR%/*}" == "$TMPDIR" ]] || exit 71
printf '%s' "$WS_DIR" > "$OBSERVED"
touch "$WS_DIR/cleanup-sentinel"
`, { shim: mktempRecorder });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, `probe failed: ${result.stderr}`);
  assert.equal(readFileSync(f.argc, 'utf8').trim(), '2',
    'mktemp must be invoked with exactly -d plus one template argument');
  assert.deepEqual(readFileSync(f.args, 'utf8').split('\n').filter(Boolean),
    ['-d', `${f.scratch}/locus-ws.XXXXXX`],
    'the full spaced path — doubled space included — is the template argument');
  const workspace = readFileSync(f.observed, 'utf8');
  assert.equal(dirname(workspace), f.scratch, 'allocated directly beneath the spaced TMPDIR');
  assert.match(workspace, /\/locus-ws\.[^/\s]+$/, 'the workspace name is one unsplit component');
  assert.ok(workspace.includes(' '), 'the allocated path must contain spaces for this test to mean anything');
  assert.throws(() => lstatSync(workspace), { code: 'ENOENT' }, 'the workspace must be removed on exit');
  assert.deepEqual(readdirSync(f.scratch).sort(), ['keep'], 'the supplied TMPDIR keeps only its own entries');
  assert.equal(readFileSync(join(f.decoy, 'decoy-sentinel'), 'utf8'), 'decoy');
});

test('cleanup under a failing exit still removes the workspace beneath a spaced TMPDIR', (t) => {
  const f = fixture(t);
  const result = probe(f, `[[ -d "$WS_DIR" ]] || exit 71
touch "$WS_DIR/cleanup-sentinel"
printf '%s' "$WS_DIR" > "$OBSERVED"
exit 42
`);

  assert.equal(result.signal, null);
  assert.equal(result.status, 42, `the probe's own failure status must surface: ${result.stderr}`);
  const workspace = readFileSync(f.observed, 'utf8');
  assert.throws(() => lstatSync(workspace), { code: 'ENOENT' }, 'the workspace must be removed on a failing exit');
  assert.deepEqual(readdirSync(f.scratch).sort(), ['keep'], 'the supplied TMPDIR keeps only its own entries');
  assert.equal(readFileSync(join(f.decoy, 'decoy-sentinel'), 'utf8'), 'decoy');
});

// The quoting regression, reintroduced: the workspace path splits on the
// spaces in TMPDIR, rm removes the first fragment — the decoy — and leaves
// the real workspace behind. This is what the decoy and removal assertions
// above, and the section 2d check inside e2e.sh, exist to detect.
test('an unquoted cleanup removes the word-split sibling instead of the workspace', (t) => {
  const f = fixture(t);
  const result = probe(f, `[[ -d "$WS_DIR" ]] || exit 71
printf '%s' "$WS_DIR" > "$OBSERVED"
`, { buggy: true });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, `the buggy probe must still run to completion: ${result.stderr}`);
  const workspace = readFileSync(f.observed, 'utf8');
  assert.equal(existsSync(join(f.decoy, 'decoy-sentinel')), false,
    'the word-split sibling is gone — the positive assertion detects this');
  assert.ok(lstatSync(workspace).isDirectory(),
    'the real workspace leaks — the removal assertion detects this');
  rmSync(workspace, { recursive: true, force: true });
});

// The same quoting regression on the creation side: an unquoted
// ${TMPDIR:-/tmp} word-splits the doubled-space root into four template
// arguments — the first fragment is the decoy sibling — instead of one. The
// recorder shows exactly what the argc and template assertions above detect;
// the split call also fails outright, so no workspace can leak.
test('an unquoted creation splits the spaced root into extra mktemp arguments', (t) => {
  const f = fixture(t);
  const buggyCreate = `create_workspace() {
  WS_DIR="$(mktemp -d \${TMPDIR:-/tmp}/locus-ws.XXXXXX)"
}`;
  const result = probe(f, ':', { shim: `${mktempRecorder}\n${buggyCreate}` });

  assert.equal(result.signal, null);
  assert.notEqual(result.status, 0, 'a word-split template cannot allocate the workspace');
  assert.equal(readFileSync(f.argc, 'utf8').trim(), '5',
    'the doubled-space root reached mktemp as four template arguments');
  const args = readFileSync(f.args, 'utf8').split('\n').filter(Boolean);
  assert.equal(args[0], '-d');
  assert.deepEqual(args.slice(1),
    [f.decoy, 'ws', 'tmp', 'root/locus-ws.XXXXXX'],
    'the doubled space collapsed — the first fragment is the decoy sibling path');
  assert.equal(readFileSync(join(f.decoy, 'decoy-sentinel'), 'utf8'), 'decoy',
    'mktemp must fail on the existing decoy, not remove or reuse it');
  assert.deepEqual(readdirSync(f.scratch).sort(), ['keep'],
    'no workspace was allocated beneath the spaced TMPDIR');
});

// The script's own section 2d rehearsal, verbatim. With intact quoting the
// decoy at the root's first word-split fragment must survive; with the
// regression reintroduced the rehearsal must stop the suite instead of
// passing a leaked workspace.
function section2d() {
  const source = readFileSync(script, 'utf8');
  const start = source.indexOf('log "2d.');
  const endMarker = 'ok "workspace creation succeeds beneath a TMPDIR containing spaces"';
  const end = source.indexOf(endMarker, start);
  assert.ok(start >= 0 && end > start, 'e2e.sh must contain the section 2d rehearsal');
  return source.slice(start, end + endMarker.length);
}

// The rehearsal calls `locus init` then `locus workspace` inside the
// workspace; the stub populates the home and writes the allowlist the way the
// real commands would. Every other subcommand fails.
function locusStub(root) {
  const bin = join(root, 'bin');
  mkdirSync(bin);
  const locus = join(bin, 'locus');
  writeFileSync(locus, `#!/bin/sh
case "$1" in
  init) printf 'stub init\\n' > "$LOCUS_HOME/init-stamp" && exit 0 ;;
  workspace) printf '# stub workspace config\\n' > .locus.toml && exit 0 ;;
esac
exit 1
`, { mode: 0o755 });
  return locus;
}

function rehearse2d(t, { buggy = false } = {}) {
  const root = mkdtempSync(join(tmpdir(), 'locus-ws-2d-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  // A space-free suite home: the first space in WS_DIR is then the one in
  // "workspace  ws temp root", so an unquoted cleanup strikes the decoy first.
  const home = join(root, 'suite-home');
  mkdirSync(home);
  const locus = locusStub(root);
  const { cleanup, create } = lifecycle();
  const override = buggy
    ? 'e2e_cleanup() { rm -rf "$LOCUS_HOME" ${WS_DIR:-} "${GRAPH_HOME:-}" "${SECURITY_HOME:-}" "${SECURITY_WS:-}"; }'
    : '';
  const result = spawnSync('bash', ['-c', `set -euo pipefail
${cleanup}
${create}
${override}
log() { :; }
ok() { printf 'ok %s\\n' "$*"; }
die() { printf 'FAIL %s\\n' "$*" >&2; exit 1; }
export LOCUS_HOME="$1"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
${section2d()}
`, 'workspace-2d', home], {
    cwd: root,
    env: { ...process.env, LOCUS_BIN: locus },
    encoding: 'utf8',
  });
  return { result, home, root };
}

test('section 2d plants a word-split decoy that survives the rehearsal', (t) => {
  const { result, home } = rehearse2d(t);
  assert.equal(result.signal, null);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /ok workspace creation succeeds beneath a TMPDIR containing spaces/);
  const decoy = join(home, 'workspace');
  const nested = join(home, 'workspace  ws temp root');
  assert.ok(existsSync(join(decoy, 'decoy-sentinel')),
    'the decoy at the word-split fragment must survive exit cleanup');
  assert.ok(lstatSync(nested).isDirectory(), 'the supplied temporary root is preserved');
  assert.deepEqual(readdirSync(nested).sort(), ['workspace-path'],
    'the rehearsed workspace is gone; only its path record remains');
  assert.throws(
    () => lstatSync(readFileSync(join(nested, 'workspace-path'), 'utf8')),
    { code: 'ENOENT' });
});

test('section 2d stops the suite when cleanup splits the spaced workspace path', (t) => {
  const { result, home } = rehearse2d(t, { buggy: true });
  assert.equal(result.signal, null);
  assert.notEqual(result.status, 0, 'the rehearsal must fail once cleanup misbehaves');
  assert.match(result.stderr, /FAIL rehearsed workspace survived exit cleanup/);
  const nested = join(home, 'workspace  ws temp root');
  assert.equal(existsSync(join(home, 'workspace', 'decoy-sentinel')), false,
    'the unquoted rm struck the word-split sibling');
  const leaked = readFileSync(join(nested, 'workspace-path'), 'utf8');
  assert.ok(lstatSync(leaked).isDirectory(), 'the unquoted rm leaked the real workspace');
  rmSync(leaked, { recursive: true, force: true });
});
