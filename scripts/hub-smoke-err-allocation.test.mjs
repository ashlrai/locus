// Run: node --test scripts/hub-smoke-err-allocation.test.mjs
// The SMOKE_ERR allocation must yield a regular file directly beneath the
// caller-supplied TMPDIR — the path mktemp prints is assigned verbatim and
// is the file the script uses. Because that file captures init/pin stderr
// diagnostics inside a shared temporary root, it must also be private
// (0600) from the instant it is allocated. The hub-smoke-spaced-home and
// hub-smoke-proj-spaced-tmpdir tests pin 0700 for the directory
// allocations; these tests pin the error file: a file, under TMPDIR,
// owner-only at allocation and at first use — even under a permissive
// umask.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { basename, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-smoke.sh');

// The mktemp wrapper verifies, the instant the real mktemp returns, that
// the allocated path is a regular empty non-symlink file directly beneath
// TMPDIR, then records the path and its mode. The locus init stub re-checks
// $SMOKE_ERR at the script's first use: same file, same parent, still
// owner-only.
const hook = `mktemp() {
  local path
  path="$(command mktemp "$@")" || return
  if [[ "$path" == "$TMPDIR"/locus-hub-smoke-err.* ]]; then
    [[ -f "$path" && ! -L "$path" && ! -s "$path" ]] || return 90
    [[ "\${path%/*}" == "$TMPDIR" ]] || return 91
    { stat -f %Lp "$path" 2>/dev/null || stat -c %a "$path"; } > "$MODE_AT_ALLOC"
    printf '%s\\n' "$path" > "$ERR_PATH"
  fi
  printf '%s\\n' "$path"
}
locus() {
  if [[ "\${1:-}" == init ]]; then
    [[ -f "$SMOKE_ERR" && ! -L "$SMOKE_ERR" && ! -s "$SMOKE_ERR" ]] || return 92
    [[ "$SMOKE_ERR" == "$(cat "$ERR_PATH")" ]] || return 93
    [[ "\${SMOKE_ERR%/*}" == "$TMPDIR" ]] || return 94
    { stat -f %Lp "$SMOKE_ERR" 2>/dev/null || stat -c %a "$SMOKE_ERR"; } > "$MODE_AT_USE"
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
  };
}

function run(f, preface = '') {
  writeFileSync(f.hook, preface + hook);
  const result = spawnSync('bash', [script], {
    env: { ...process.env, BASH_ENV: f.hook, TMPDIR: f.scratch,
      ERR_PATH: f.errPath, MODE_AT_ALLOC: f.modeAtAlloc, MODE_AT_USE: f.modeAtUse },
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
});
