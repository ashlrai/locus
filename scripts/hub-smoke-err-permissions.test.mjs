// Run: node --test scripts/hub-smoke-err-permissions.test.mjs
// SMOKE_ERR captures init/pin stderr diagnostics, so creation under a
// space-containing TMPDIR must not only land at the expected path — it must
// land there owner-private. mktemp allocates with mode 0600 regardless of
// umask; this test runs the script under `umask 000`, where a naive
// redirect-or-truncate implementation would produce a world-accessible file.
// A control file created by plain redirection proves the umask really was
// permissive, so the 0600 observed on the error file can only come from
// mktemp. A second test anchors ordering: the file must already exist at its
// final path when the script runs its first non-mktemp external command
// (od, generating LOCUS_CONTROL_CAPABILITY).
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { basename, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-smoke.sh');

test('SMOKE_ERR is created owner-private at the expected path under a spaced TMPDIR, despite a permissive umask', (t) => {
  const root = mkdtempSync('/tmp/lhs-err-perms-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'err tmp dir');
  const created = join(root, 'err-created');
  const used = join(root, 'err-used');
  const control = join(root, 'control-file');
  const hook = join(root, 'hook.sh');
  mkdirSync(scratch);
  // The mktemp shim verifies, the instant the real mktemp returns, that the
  // error file is a regular empty file directly beneath the spaced TMPDIR
  // with mode 600 — then lays down a control file via plain redirection so
  // the test can prove umask was 000. The locus init stub re-checks the same
  // path at first use: still mode 600, still owner-writable. stat flag
  // portability follows dogfood.test.mjs: BSD -f %Lp, GNU -c %a.
  writeFileSync(hook, `umask 000
mktemp() {
  local path mode
  path="$(command mktemp "$@")" || return
  if [[ "$path" == "$TMPDIR"/locus-hub-smoke-err.* ]]; then
    [[ -f "$path" && ! -L "$path" && ! -s "$path" ]] || { echo "mktemp-shim: not a regular empty file" >&2; return 70; }
    if [[ "$(uname -s)" == Darwin ]]; then mode="$(stat -f %Lp "$path")"; else mode="$(stat -c %a "$path")"; fi
    [[ "$mode" == 600 ]] || { echo "mktemp-shim: mode $mode" >&2; return 71; }
    printf '%s\\n' "$path" > "$ERR_CREATED"
    : > "$CONTROL_CREATED"
  fi
  printf '%s\\n' "$path"
}
locus() {
  if [[ "\${1:-}" == init ]]; then
    [[ "$SMOKE_ERR" == "$(cat "$ERR_CREATED")" ]] || { echo "init-stub: path moved" >&2; return 91; }
    [[ -f "$SMOKE_ERR" && ! -L "$SMOKE_ERR" && -w "$SMOKE_ERR" ]] || { echo "init-stub: not a writable file" >&2; return 92; }
    local mode
    if [[ "$(uname -s)" == Darwin ]]; then mode="$(stat -f %Lp "$SMOKE_ERR")"; else mode="$(stat -c %a "$SMOKE_ERR")"; fi
    [[ "$mode" == 600 ]] || { echo "init-stub: mode $mode" >&2; return 93; }
    : > "$ERR_USED"
  fi
  if [[ "\${1:-} \${2:-}" == 'status --oneline' ]]; then
    printf '%s\\n' personal:tenant
  else
    printf '%s\\n' '{}'
  fi
}
jq() { cat >/dev/null; printf '%s\\n' grant; }
cargo() { return 0; }
`);
  const result = spawnSync('bash', [script], {
    env: { ...process.env, BASH_ENV: hook, TMPDIR: scratch,
      ERR_CREATED: created, ERR_USED: used, CONTROL_CREATED: control },
    encoding: 'utf8',
    timeout: 15_000,
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 0, `stderr:\n${result.stderr}`);
  assert.match(result.stdout, /^== hub-smoke OK ==$/m);
  assert.doesNotMatch(result.stdout, /^skip\s/m, 'all smoke checks must run');
  assert.ok(existsSync(used), 'init must observe the error file at its recorded path');

  const errorFile = readFileSync(created, 'utf8').trim();
  assert.equal(dirname(errorFile), scratch, 'error file must be a direct child of the spaced TMPDIR');
  assert.match(basename(errorFile), /^locus-hub-smoke-err\.[A-Za-z0-9]{6}$/);
  // The control file was made by plain redirection under umask 000, so its
  // 0666 proves mktemp — not ambient umask — produced the file's 0600.
  assert.equal(statSync(control).mode & 0o777, 0o666, 'control must be world-accessible under umask 000');
  // The EXIT trap removed the error file; TMPDIR is empty again.
  assert.equal(existsSync(errorFile), false, 'temporary error file must be removed');
  assert.deepEqual(readdirSync(scratch), []);
});

test('SMOKE_ERR already exists at its expected path when the first non-mktemp command runs', (t) => {
  const root = mkdtempSync('/tmp/lhs-err-early-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'early tmp dir');
  const created = join(root, 'err-created');
  const early = join(root, 'err-early');
  const used = join(root, 'err-used');
  const hook = join(root, 'hook.sh');
  mkdirSync(scratch);
  // od (in the LOCUS_CONTROL_CAPABILITY pipeline) is the script's first
  // external command after the three mktemp allocations. The shim records
  // $SMOKE_ERR only if the file already exists directly beneath TMPDIR, so
  // a lazy or reordered allocation would leave no record. The init stub
  // then confirms the same file is the one the script goes on to use.
  writeFileSync(hook, `od() {
  if [[ -n "\${SMOKE_ERR:-}" && -f "$SMOKE_ERR" && "\${SMOKE_ERR%/*}" == "$TMPDIR" ]]; then
    printf '%s\\n' "$SMOKE_ERR" > "$ERR_EARLY"
  fi
  command od "$@"
}
mktemp() {
  local path
  path="$(command mktemp "$@")" || return
  if [[ "$path" == "$TMPDIR"/locus-hub-smoke-err.* ]]; then
    printf '%s\\n' "$path" > "$ERR_CREATED"
  fi
  printf '%s\\n' "$path"
}
locus() {
  if [[ "\${1:-}" == init ]]; then
    [[ "$SMOKE_ERR" == "$(cat "$ERR_CREATED")" ]] || { echo "init-stub: path moved" >&2; return 91; }
    printf '%s\\n' "$SMOKE_ERR" > "$ERR_USED"
  fi
  if [[ "\${1:-} \${2:-}" == 'status --oneline' ]]; then
    printf '%s\\n' personal:tenant
  else
    printf '%s\\n' '{}'
  fi
}
jq() { cat >/dev/null; printf '%s\\n' grant; }
cargo() { return 0; }
`);
  const result = spawnSync('bash', [script], {
    env: { ...process.env, BASH_ENV: hook, TMPDIR: scratch,
      ERR_CREATED: created, ERR_EARLY: early, ERR_USED: used },
    encoding: 'utf8',
    timeout: 15_000,
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 0, `stderr:\n${result.stderr}`);
  assert.match(result.stdout, /^== hub-smoke OK ==$/m);
  assert.doesNotMatch(result.stdout, /^skip\s/m, 'all smoke checks must run');

  const errorFile = readFileSync(created, 'utf8').trim();
  assert.equal(dirname(errorFile), scratch, 'error file must be a direct child of the spaced TMPDIR');
  assert.match(basename(errorFile), /^locus-hub-smoke-err\.[A-Za-z0-9]{6}$/);
  assert.ok(existsSync(early), 'the file must exist before the first non-mktemp command runs');
  assert.equal(readFileSync(early, 'utf8').trim(), errorFile, 'od must see the same file mktemp created');
  assert.equal(readFileSync(used, 'utf8').trim(), errorFile, 'init must use the same file');
  assert.equal(existsSync(errorFile), false, 'temporary error file must be removed');
  assert.deepEqual(readdirSync(scratch), []);
});
