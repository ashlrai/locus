// Run: node --test scripts/hub-smoke-err-cleanup.test.mjs
// A successful hub-smoke run must remove the SMOKE_ERR capture file itself —
// not only the sibling home and project directories. The file exists for the
// whole run so late init/pin retries still have somewhere to write stderr,
// then the EXIT trap deletes exactly that path: same-prefix decoys and the
// TMPDIR itself are untouched. hub-smoke-error-cleanup.test.mjs pins the
// populated file's content at removal; these tests pin the removal on the
// success path for a populated file and for one that never captured output.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { basename, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-smoke.sh');

// Stubs every external command so only the error-file lifecycle is exercised.
// The mktemp shim records the allocated SMOKE_ERR path; the locus shim logs a
// violation whenever the file is missing during the run, reports its size at
// the final external call (`mcp list`, last in the script's grant block), and
// — where DIAG=1 — writes synthetic init/pin diagnostics to stderr so the
// script's own `2>"$SMOKE_ERR"` redirections populate the file.
const hook = `mktemp() {
  local path
  path="$(command mktemp "$@")" || return
  case "$path" in
    "$TMPDIR"/locus-hub-smoke-err.*) printf '%s\\n' "$path" > "$ERR_PATH" ;;
  esac
  printf '%s\\n' "$path"
}
jq() { cat >/dev/null; printf '%s\\n' grant; }
cargo() { return 0; }
locus() {
  [[ -f "$SMOKE_ERR" ]] || printf 'missing during: %s\\n' "$*" >> "$VIOLATIONS"
  case "\${1:-} \${2:-}" in
    'init --with-samples')
      [[ -f "$SMOKE_ERR" && ! -s "$SMOKE_ERR" ]] || return 90
      [[ "\${DIAG:-}" == 1 ]] && printf 'synthetic init diagnostic\\n' >&2 ;;
    'pin personal')
      [[ -f "$SMOKE_ERR" && ! -s "$SMOKE_ERR" \\
        && "$SMOKE_ERR" == "$TMPDIR"/locus-hub-smoke-err.* ]] || return 91
      [[ "\${DIAG:-}" == 1 ]] && printf 'synthetic pin diagnostic\\n' >&2 ;;
    'status --oneline') printf '%s\\n' personal:tenant; return 0 ;;
    'mcp list')
      if [[ -s "$SMOKE_ERR" ]]; then printf 'nonempty\\n'; else printf 'empty\\n'; fi >> "$SIZE_LOG" ;;
  esac
  printf '%s\\n' '{}'
}
`;

function runSmoke(t, { diag, tmpdirName }) {
  const root = mkdtempSync('/tmp/lhs-errcl-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, tmpdirName);
  const errPath = join(root, 'err-path');
  const violations = join(root, 'violations');
  const sizeLog = join(root, 'size-log');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  writeFileSync(join(scratch, 'locus-hub-smoke-err.decoy'), 'sentinel');
  writeFileSync(join(root, 'hook.sh'), hook);

  const result = spawnSync('bash', [script], {
    env: { ...process.env, TMPDIR: scratch, BASH_ENV: join(root, 'hook.sh'),
      ERR_PATH: errPath, VIOLATIONS: violations, SIZE_LOG: sizeLog, DIAG: diag },
    encoding: 'utf8',
    timeout: 15_000,
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 0, `smoke must complete successfully:\n${result.stdout}${result.stderr}`);
  assert.ok(result.stdout.split('\n').includes('== hub-smoke OK =='), 'success banner missing');
  assert.doesNotMatch(result.stdout, /^skip\s/m, 'all smoke checks must run');
  const errFile = readFileSync(errPath, 'utf8').trim();
  assert.equal(dirname(errFile), scratch);
  assert.ok(basename(errFile).startsWith('locus-hub-smoke-err.'));
  const missing = existsSync(violations) ? readFileSync(violations, 'utf8') : '';
  assert.equal(missing, '', 'error file must stay live for the whole run');
  assert.equal(existsSync(errFile), false, 'temporary error file must be removed on successful exit');
  assert.deepEqual(readdirSync(scratch).sort(), ['keep', 'locus-hub-smoke-err.decoy'],
    'only the run-owned error file may be removed');
  assert.equal(readFileSync(join(scratch, 'locus-hub-smoke-err.decoy'), 'utf8'), 'sentinel');
  assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'unrelated');
  return { sizeLog };
}

test('successful exit removes the populated error file after the last command', (t) => {
  const { sizeLog } = runSmoke(t, { diag: '1', tmpdirName: 'captured diagnostics' });
  // The final `mcp list` call saw the file still holding pin's stderr; the
  // EXIT trap — not an intermediate step — is what removes it.
  assert.equal(readFileSync(sizeLog, 'utf8').trimEnd().split('\n').at(-1), 'nonempty');
});

test('successful exit removes the error file that never captured diagnostics', (t) => {
  const { sizeLog } = runSmoke(t, { diag: '', tmpdirName: 'silent run' });
  assert.equal(readFileSync(sizeLog, 'utf8').trimEnd().split('\n').at(-1), 'empty');
});
