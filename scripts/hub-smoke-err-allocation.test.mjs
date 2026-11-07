// Run: node --test scripts/hub-smoke-err-allocation.test.mjs
// The SMOKE_ERR allocation must yield a regular file directly beneath the
// caller-supplied TMPDIR — the path mktemp prints is assigned verbatim and
// is the file the script uses. Because that file captures init/pin stderr
// diagnostics inside a shared temporary root, it must also be private
// (0600) from the instant it is allocated. The hub-smoke-spaced-home and
// hub-smoke-proj-spaced-tmpdir tests pin 0700 for the directory
// allocations; these tests pin the error file: a file, under TMPDIR,
// owner-only at allocation and at first use — even under a permissive
// umask. The permissive-umask run also drops a control file by plain
// redirection: its 0666 can only come from the umask, proving mktemp —
// not ambient permissions — produced the error file's 0600. A further
// test anchors ordering: the file must already exist at its final path
// when the script runs its first non-mktemp external command (od, in the
// LOCUS_CONTROL_CAPABILITY pipeline), and init must use that same file.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { basename, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-smoke.sh');

// The mktemp wrapper verifies, the instant the real mktemp returns, that
// the allocated path is a regular empty non-symlink file directly beneath
// TMPDIR, then records the path and its mode; when CONTROL_PATH is set it
// also drops a control file by plain redirection so a permissive-umask run
// can prove the umask really was permissive. The od wrapper — od is the
// script's first non-mktemp external command after the three allocations —
// records $SMOKE_ERR if the file already exists at its final path beneath
// TMPDIR. The locus init stub re-checks $SMOKE_ERR at the script's first
// use: same file, same parent, still owner-only, and records it as used.
const hook = `mktemp() {
  local path
  path="$(command mktemp "$@")" || return
  if [[ "$path" == "$TMPDIR"/locus-hub-smoke-err.* ]]; then
    [[ -f "$path" && ! -L "$path" && ! -s "$path" ]] || return 90
    [[ "\${path%/*}" == "$TMPDIR" ]] || return 91
    { stat -f %Lp "$path" 2>/dev/null || stat -c %a "$path"; } > "$MODE_AT_ALLOC"
    printf '%s\\n' "$path" > "$ERR_PATH"
    if [[ -n "\${CONTROL_PATH:-}" ]]; then : > "$CONTROL_PATH"; fi
  fi
  printf '%s\\n' "$path"
}
od() {
  if [[ -n "\${ERR_EARLY:-}" && -n "\${SMOKE_ERR:-}" && -f "$SMOKE_ERR" \\
    && "\${SMOKE_ERR%/*}" == "$TMPDIR" ]]; then
    printf '%s\\n' "$SMOKE_ERR" > "$ERR_EARLY"
  fi
  command od "$@"
}
locus() {
  if [[ "\${1:-}" == init ]]; then
    [[ -f "$SMOKE_ERR" && ! -L "$SMOKE_ERR" && ! -s "$SMOKE_ERR" ]] || return 92
    [[ "$SMOKE_ERR" == "$(cat "$ERR_PATH")" ]] || return 93
    [[ "\${SMOKE_ERR%/*}" == "$TMPDIR" ]] || return 94
    { stat -f %Lp "$SMOKE_ERR" 2>/dev/null || stat -c %a "$SMOKE_ERR"; } > "$MODE_AT_USE"
    if [[ -n "\${ERR_USED:-}" ]]; then printf '%s\\n' "$SMOKE_ERR" > "$ERR_USED"; fi
  fi
  if [[ "\${1:-} \${2:-}" == 'status --oneline' ]]; then
    printf '%s\\n' personal:tenant
  else
    printf '%s\\n' '{}'
  fi
}
jq() { cat >/dev/null; printf '%s\\n' grant; }
cargo() { return 0; }
`;

function fixture(t) {
  const root = mkdtempSync('/tmp/lhs-err-alloc-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'supplied tmpdir');
  mkdirSync(scratch);
  return {
    root,
    scratch,
    hook: join(root, 'hook.sh'),
    errPath: join(root, 'err-path'),
    modeAtAlloc: join(root, 'mode-alloc'),
    modeAtUse: join(root, 'mode-use'),
    early: join(root, 'err-early'),
    used: join(root, 'err-used'),
    control: join(root, 'control-file'),
  };
}

function run(f, preface = '') {
  writeFileSync(f.hook, preface + hook);
  const env = { ...process.env, BASH_ENV: f.hook, TMPDIR: f.scratch,
    ERR_PATH: f.errPath, MODE_AT_ALLOC: f.modeAtAlloc, MODE_AT_USE: f.modeAtUse,
    ERR_EARLY: f.early, ERR_USED: f.used, CONTROL_PATH: f.control };
  // The capability pipeline only runs od when the variable is unset; drop an
  // inherited value so the ordering probe cannot be silently skipped.
  delete env.LOCUS_CONTROL_CAPABILITY;
  const result = spawnSync('bash', [script], {
    env,
    encoding: 'utf8',
    timeout: 15_000,
  });
  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 0, `stderr:\n${result.stderr}`);
  assert.match(result.stdout, /^== hub-smoke OK ==$/m);
  assert.doesNotMatch(result.stdout, /^skip\s/m, 'all smoke checks must run');
  return result;
}

test('the allocated SMOKE_ERR path is a file directly beneath the supplied TMPDIR', (t) => {
  const f = fixture(t);
  writeFileSync(join(f.scratch, 'keep'), 'sentinel');
  run(f);
  const allocated = readFileSync(f.errPath, 'utf8').trim();
  assert.equal(dirname(allocated), f.scratch, 'allocated path must sit directly beneath TMPDIR');
  assert.match(basename(allocated), /^locus-hub-smoke-err\.[A-Za-z0-9]{6}$/);
  assert.equal(existsSync(allocated), false, 'the error file must be removed on exit');
  assert.deepEqual(readdirSync(f.scratch), ['keep'], 'TMPDIR keeps only its own entries');
});

test('the allocated SMOKE_ERR file is private (0600) at allocation and first use', (t) => {
  const f = fixture(t);
  run(f);
  assert.equal(readFileSync(f.modeAtAlloc, 'utf8').trim(), '600',
    'mktemp must allocate the error file owner-only');
  assert.equal(readFileSync(f.modeAtUse, 'utf8').trim(), '600',
    'the error file must stay owner-only until first use');
});

test('the allocation stays private under a permissive umask', (t) => {
  const f = fixture(t);
  run(f, 'umask 000\n');
  assert.equal(readFileSync(f.modeAtAlloc, 'utf8').trim(), '600');
  assert.equal(readFileSync(f.modeAtUse, 'utf8').trim(), '600');
  // The control file was made by plain redirection under umask 000, so its
  // 0666 proves mktemp — not ambient umask — produced the file's 0600.
  assert.equal(statSync(f.control).mode & 0o777, 0o666,
    'control must be world-accessible under umask 000');
});

test('SMOKE_ERR exists at its final path before the first non-mktemp command runs', (t) => {
  const f = fixture(t);
  run(f);
  const created = readFileSync(f.errPath, 'utf8').trim();
  assert.equal(dirname(created), f.scratch, 'error file must be a direct child of TMPDIR');
  assert.match(basename(created), /^locus-hub-smoke-err\.[A-Za-z0-9]{6}$/);
  assert.ok(existsSync(f.early), 'the file must exist before the first non-mktemp command runs');
  assert.equal(readFileSync(f.early, 'utf8').trim(), created,
    'od must see the same file mktemp created');
  assert.equal(readFileSync(f.used, 'utf8').trim(), created,
    'init must use the same file');
  assert.equal(existsSync(created), false, 'temporary error file must be removed on exit');
  assert.deepEqual(readdirSync(f.scratch), []);
});
