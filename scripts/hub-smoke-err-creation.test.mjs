// Run: node --test scripts/hub-smoke-err-creation.test.mjs
// Under a space-containing TMPDIR, mktemp must create SMOKE_ERR as one real
// file at the intended path — not split into a parent fragment plus a
// relative remainder. Decoy directories give a hypothetical split somewhere
// to land, so the run must still leave exactly one error file: the recorded
// one.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { basename, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-smoke.sh');

test('mktemp creates SMOKE_ERR as a file at the intended path under a spaced TMPDIR', (t) => {
  const root = mkdtempSync('/tmp/lhs-err-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'tmp dir');
  const decoy = join(root, 'tmp');
  const cwd = join(root, 'cwd');
  const record = join(root, 'err-path');
  const observed = join(root, 'err-observed');
  const hook = join(root, 'hook.sh');
  mkdirSync(scratch);
  mkdirSync(decoy);
  mkdirSync(join(cwd, 'dir'), { recursive: true });
  writeFileSync(join(decoy, 'sentinel'), 'keep');
  // The mktemp shim verifies, the instant the real mktemp returns, that a
  // regular empty file exists at the returned path and records it. The locus
  // init stub then re-checks $SMOKE_ERR when the script first uses it: still
  // the same file, still a direct child of TMPDIR, and the only
  // locus-hub-smoke-err.* entry there.
  writeFileSync(hook, `mktemp() {
  local path
  path="$(command mktemp "$@")" || return
  if [[ "$path" == "$TMPDIR"/locus-hub-smoke-err.* ]]; then
    [[ -f "$path" && ! -L "$path" && ! -s "$path" ]] || return 90
    printf '%s\\n' "$path" > "$ERR_PATH"
  fi
  printf '%s\\n' "$path"
}
locus() {
  if [[ "\${1:-}" == init ]]; then
    [[ -f "$SMOKE_ERR" && "\${SMOKE_ERR%/*}" == "$TMPDIR" ]] || return 91
    [[ "$SMOKE_ERR" == "$(cat "$ERR_PATH")" ]] || return 92
    local entries=("$TMPDIR"/locus-hub-smoke-err.*)
    [[ \${#entries[@]} -eq 1 && "\${entries[0]}" == "$SMOKE_ERR" ]] || return 93
    : > "$ERR_OBSERVED"
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
    cwd,
    env: { ...process.env, BASH_ENV: hook, TMPDIR: scratch,
      ERR_PATH: record, ERR_OBSERVED: observed },
    encoding: 'utf8',
    timeout: 15_000,
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 0, `stderr:\n${result.stderr}`);
  assert.match(result.stdout, /^== hub-smoke OK ==$/m);
  assert.doesNotMatch(result.stdout, /^skip\s/m, 'all smoke checks must run');
  assert.ok(existsSync(observed), 'init must see the error file at its recorded path');
  const errorFile = readFileSync(record, 'utf8').trim();
  assert.equal(dirname(errorFile), scratch, 'error file must be a direct child of TMPDIR');
  assert.match(basename(errorFile), /^locus-hub-smoke-err\..{6}$/);
  // Nothing split: the decoy survives and no fragment landed under cwd.
  assert.equal(readFileSync(join(decoy, 'sentinel'), 'utf8'), 'keep');
  assert.deepEqual(readdirSync(cwd), ['dir']);
  assert.deepEqual(readdirSync(join(cwd, 'dir')), []);
  // The EXIT trap removed the file; TMPDIR is empty.
  assert.equal(existsSync(errorFile), false, 'temporary error file must be removed');
  assert.deepEqual(readdirSync(scratch), []);
});
