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
mktemp() {
  local last="\${!#}"
  if [[ "$last" == "$TMPDIR"/locus-hub-int.XXXXXX ]]; then
    [[ $# -eq 2 && "\${1:-}" == -d ]] || return 71
    printf 'mktemp %s %s\\n' "$#" "$last" >> "$SMOKE_ARGV"
  fi
  command mktemp "$@"
}
rm() {
  local arg
  for arg in "$@"; do
    [[ "$arg" == "$TMPDIR"/* ]] && printf 'rm %s %s\\n' "$#" "$arg" >> "$SMOKE_ARGV"
  done
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
export -f node jq cargo mktemp rm locus
exec bash "$1"
`;

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
  return { root, scratch, home, argv: readFileSync(record, 'utf8').trim().split('\n') };
}

test('mktemp receives the SMOKE_HOME template as one argument beneath a space-containing TMPDIR', (t) => {
  const { scratch, home, argv } = run(t, 'home workspace dir');

  // mktemp was called once for SMOKE_HOME, with exactly -d and the full
  // template — the spaced TMPDIR survived as a single argument.
  const mk = argv.filter((line) => line.startsWith('mktemp '));
  assert.equal(mk.length, 1, 'mktemp must be called once for SMOKE_HOME');
  const mkParts = mk[0].split(' ');
  assert.equal(mkParts[1], '2', 'mktemp receives -d and the full template as two arguments');
  assert.equal(mkParts.slice(2).join(' '), `${scratch}/locus-hub-int.XXXXXX`);

  // The directory materialized directly beneath the exact requested root.
  assert.equal(dirname(home), scratch);
  assert.match(basename(home), /^locus-hub-int\.[A-Za-z0-9]{6}$/);

  // The EXIT trap removes the scan-listing glob and both directories: each
  // argument reaches rm unsplit, the populated home inside the one rm -rf.
  const rmLines = argv.filter((line) => line.startsWith('rm '));
  assert.equal(rmLines.length, 3, 'cleanup passes the scan glob, home and project to rm');
  assert.ok(rmLines.some((line) => line.includes('.locus-hub-int-scan.')),
    'cleanup must pass the scan-listing glob beneath TMPDIR to rm');
  const rmHome = rmLines.find((line) => line.includes('locus-hub-int.') && !line.includes('-proj.'));
  assert.ok(rmHome, 'cleanup must pass SMOKE_HOME to rm');
  const rmParts = rmHome.split(' ');
  assert.equal(rmParts[1], '4', 'rm -rf -- home proj: four arguments total');
  assert.equal(rmParts.slice(2).join(' '), home, 'the populated home reaches rm verbatim');

  assert.equal(existsSync(home), false, 'the populated SMOKE_HOME must be removed on exit');
  assert.deepEqual(readdirSync(scratch), ['sentinel'], 'the requested root keeps only its own entries');
  assert.equal(readFileSync(join(scratch, 'sentinel'), 'utf8'), 'keep');
});

test('SMOKE_HOME lands beneath the exact requested root, not a word-split fragment', (t) => {
  // Consecutive spaces must be preserved verbatim: the directory's parent is
  // the requested root itself — nothing collapsed, split or canonicalized —
  // and nothing may land beneath 'tmp', the first word-split fragment.
  const { root, scratch, home, argv } = run(t, 'tmp  dir', ['tmp']);

  const mk = argv.filter((line) => line.startsWith('mktemp '));
  assert.equal(mk.length, 1, 'mktemp must be called once for SMOKE_HOME');
  assert.equal(mk[0].split(' ').slice(2).join(' '), `${scratch}/locus-hub-int.XXXXXX`,
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

  const mk = argv.filter((line) => line.startsWith('mktemp '));
  assert.equal(mk.length, 1);
  assert.equal(mk[0].split(' ').slice(2).join(' '), `${scratch}/locus-hub-int.XXXXXX`);
  assert.equal(dirname(home), scratch);
  assert.match(basename(home), /^locus-hub-int\.[A-Za-z0-9]{6}$/);
  assert.equal(existsSync(home), false);
  assert.deepEqual(readdirSync(scratch), ['sentinel']);
});
