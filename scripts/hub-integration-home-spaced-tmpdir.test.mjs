// Run: node --test scripts/hub-integration-home-spaced-tmpdir.test.mjs
// SMOKE_HOME must be created beneath the caller-selected TMPDIR even when the
// root contains spaces: mktemp -d must receive the full template as one
// argument, the directory must materialize directly beneath the exact
// requested root (no word-split fragment, no collapsed whitespace), become
// LOCUS_HOME for the run, and reach the EXIT trap's rm as a single argument.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { basename, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-integration-test.sh');

// Stub the prerequisites; mktemp and rm record their argv so the test can
// verify the spaced TMPDIR is never word-split. The mktemp shim fails fast
// unless SMOKE_HOME's template arrives as the second of exactly two
// arguments. The init stub re-verifies the created directory in place — a
// real, non-symlink, owner-only directory whose parent is the verbatim
// $TMPDIR — populates it through the complete spaced path, records it, and
// stops the run, which drives the EXIT trap and its cleanup.
const harness = `
node() { :; }
jq() { :; }
cargo() { :; }
record_argv() {
  printf '%s\\0' "$@" >> "$SMOKE_ARGV"
}
mktemp() {
  record_argv mktemp "$#" "$@"
  local last="\${!#}"
  if [[ "$last" == "$TMPDIR"/locus-hub-int.XXXXXX ]]; then
    [[ $# -eq 2 && "\${1:-}" == -d ]] || return 71
  fi
  command mktemp "$@"
}
rm() {
  record_argv rm "$#" "$@"
  command rm "$@"
}
locus() {
  if [[ "\${1:-}" == init ]]; then
    [[ -d "$SMOKE_HOME" && ! -L "$SMOKE_HOME" ]] || return 74
    [[ "$SMOKE_HOME" == "$LOCUS_HOME" ]] || return 75
    [[ "\${SMOKE_HOME%/*}" == "$TMPDIR" ]] || return 76
    [[ "\${SMOKE_HOME##*/}" == locus-hub-int.?????? ]] || return 77
    [[ "$(stat -f %Lp "$SMOKE_HOME" 2>/dev/null || stat -c %a "$SMOKE_HOME")" == 700 ]] || return 78
    mkdir -p "$SMOKE_HOME/nested dir" && printf payload > "$SMOKE_HOME/nested dir/f.toml" || return 79
    printf '%s\\n' "$SMOKE_HOME" > "$SMOKE_HOME_RECORD"
    return 73
  fi
}
export -f node jq cargo record_argv mktemp rm locus
exec bash "$1"
`;

// NUL framing preserves every argument verbatim, including whitespace and
// newlines; the count separates successive invocations without shell parsing.
function readArgv(record) {
  const fields = readFileSync(record, 'utf8').split('\0');
  assert.equal(fields.pop(), '', 'every recorded argument must be terminated');
  const calls = [];
  while (fields.length) {
    const command = fields.shift();
    const count = Number(fields.shift());
    assert.ok(Number.isInteger(count) && count > 0 && count <= fields.length);
    calls.push({ command, args: fields.splice(0, count) });
  }
  return calls;
}

// decoys: directory names placed beside the requested root. A word-splitting
// regression could let a locus-hub-int.* fragment land in one of them.
function run(t, scratchName, decoys = []) {
  const root = mkdtempSync('/tmp/lhi-home-spaced-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, scratchName);
  const record = join(root, 'argv');
  const homeFile = join(root, 'home');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'sentinel'), 'keep');
  for (const name of decoys) {
    const decoy = join(root, name);
    mkdirSync(decoy);
    writeFileSync(join(decoy, 'keep'), 'sentinel');
  }
  const result = spawnSync('bash', ['-c', harness, 'spaced-home', script], {
    env: { PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: scratch,
      SMOKE_ARGV: record, SMOKE_HOME_RECORD: homeFile },
    encoding: 'utf8',
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 73,
    `init must observe the live home then stop: stderr:\n${result.stderr}\nstdout:\n${result.stdout}`);
  const home = readFileSync(homeFile, 'utf8').trim();
  assert.ok(result.stdout.includes(`== hub-integration-test LOCUS_HOME=${home} ==`),
    'the recorded directory must be the exported LOCUS_HOME');
  return { root, scratch, home, argv: readArgv(record) };
}

test('mktemp receives the SMOKE_HOME template as one argument beneath a space-containing TMPDIR', (t) => {
  const { scratch, home, argv } = run(t, 'home workspace dir');

  // The first mktemp call allocates SMOKE_HOME, with exactly -d and the full
  // template — the spaced TMPDIR survived as a single argument.
  const mk = argv.filter((call) => call.command === 'mktemp');
  assert.equal(mk.length, 2, 'mktemp must allocate the home and project');
  assert.deepEqual(mk[0].args, ['-d', `${scratch}/locus-hub-int.XXXXXX`]);
  assert.deepEqual(mk[1].args, ['-d', `${scratch}/locus-hub-int-proj.XXXXXX`]);

  // The directory materialized directly beneath the exact requested root.
  assert.equal(dirname(home), scratch);
  assert.match(basename(home), /^locus-hub-int\.[A-Za-z0-9]{6}$/);

  // The EXIT trap removes the scan-listing glob and both directories: each
  // argument reaches rm unsplit, the populated home inside the one rm -rf.
  const removals = argv.filter((call) => call.command === 'rm');
  assert.equal(removals.length, 2, 'cleanup removes scan listings then both directories');
  assert.deepEqual(removals[0].args.slice(0, 2), ['-f', '--']);
  assert.equal(removals[0].args.length, 3);
  assert.equal(dirname(removals[0].args[2]), scratch);
  assert.match(basename(removals[0].args[2]), /^\.locus-hub-int-scan\.\d+\.\*$/);
  const project = removals[1].args[3];
  assert.equal(dirname(project), scratch);
  assert.match(basename(project), /^locus-hub-int-proj\.[A-Za-z0-9]{6}$/);
  assert.deepEqual(removals[1].args, ['-rf', '--', home, project],
    'the populated home reaches rm verbatim as exactly one argument');

  assert.equal(existsSync(home), false, 'the populated SMOKE_HOME must be removed on exit');
  assert.deepEqual(readdirSync(scratch), ['sentinel'], 'the requested root keeps only its own entries');
  assert.equal(readFileSync(join(scratch, 'sentinel'), 'utf8'), 'keep');
});

test('SMOKE_HOME lands beneath the exact requested root, not a word-split fragment', (t) => {
  // Consecutive spaces must be preserved verbatim: the directory's parent is
  // the requested root itself — nothing collapsed, split or canonicalized —
  // and nothing may land beneath 'tmp', the first word-split fragment.
  const { root, scratch, home, argv } = run(t, 'tmp  dir', ['tmp']);

  const mk = argv.filter((call) => call.command === 'mktemp');
  assert.equal(mk.length, 2, 'mktemp must allocate the home and project');
  assert.deepEqual(mk[0].args, ['-d', `${scratch}/locus-hub-int.XXXXXX`],
    'the template keeps the consecutive spaces of the requested root');
  assert.equal(dirname(home), scratch);
  assert.match(basename(home), /^locus-hub-int\.[A-Za-z0-9]{6}$/);
  assert.equal(existsSync(home), false, 'SMOKE_HOME must be removed on exit');

  const decoy = join(root, 'tmp');
  assert.deepEqual(readdirSync(decoy), ['keep'],
    'no locus-hub-int.* fragment may land beneath the split-off parent');
  assert.equal(readFileSync(join(decoy, 'keep'), 'utf8'), 'sentinel');
  assert.deepEqual(readdirSync(scratch), ['sentinel']);
});

test('SMOKE_HOME stays under a spaced TMPDIR containing shell metacharacters', (t) => {
  const { scratch, home, argv } = run(t, 'home [*] workspace');

  const mk = argv.filter((call) => call.command === 'mktemp');
  assert.equal(mk.length, 2);
  assert.deepEqual(mk[0].args, ['-d', `${scratch}/locus-hub-int.XXXXXX`]);
  assert.equal(dirname(home), scratch);
  assert.match(basename(home), /^locus-hub-int\.[A-Za-z0-9]{6}$/);
  assert.equal(existsSync(home), false);
  assert.deepEqual(readdirSync(scratch), ['sentinel']);
});


test('argument records preserve tabs and newlines within a spaced temporary home', (t) => {
  const { scratch, home, argv } = run(t, 'home \twith\nspaces');
  const allocations = argv.filter((call) => call.command === 'mktemp');
  assert.deepEqual(allocations[0].args, ['-d', `${scratch}/locus-hub-int.XXXXXX`]);
  const cleanup = argv.find((call) => call.command === 'rm' && call.args[0] === '-rf');
  assert.equal(cleanup.args.length, 4);
  assert.equal(cleanup.args[2], home, 'whitespace stays inside one home argument');
  assert.equal(existsSync(home), false);
  assert.deepEqual(readdirSync(scratch), ['sentinel']);
});
