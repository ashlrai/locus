// Run: node --test scripts/hub-smoke-error-cleanup.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-smoke.sh');

test('normal completion removes the populated temporary error file', (t) => {
  const root = mkdtempSync('/tmp/locus-err-test-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'temporary files');
  const record = join(root, 'error-file');
  const observed = join(root, 'observed');
  const hook = join(root, 'hook.sh');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'locus-hub-smoke-err.unrelated'), 'keep');
  // init and pin write synthetic diagnostics to stderr so the run leaves a
  // populated error file for the EXIT trap to remove.
  writeFileSync(hook, `mktemp() {
  local path
  path="$(command mktemp "$@")" || return
  case "$path" in
    "$TMPDIR"/locus-hub-smoke-err.*) printf '%s\\n' "$path" > "$ERR_RECORD" ;;
  esac
  printf '%s\\n' "$path"
}
jq() { printf '%s\\n' grant; }
cargo() { return 0; }
locus() {
  if [[ "\${1:-} \${2:-}" == 'status --oneline' ]]; then
    printf '%s\\n' personal:tenant
    return 0
  fi
  case "\${1:-}" in
    init|pin)
      [[ -f "$SMOKE_ERR" && ! -s "$SMOKE_ERR" \\
        && "$SMOKE_ERR" == "$TMPDIR"/locus-hub-smoke-err.* ]] || return 90
      printf 'synthetic %s diagnostic\\n' "$1" >&2 ;;
  esac
  printf '%s\\n' '{}'
}
rm() {
  local errfile arg
  errfile="$(cat "$ERR_RECORD")"
  for arg in "$@"; do
    if [[ "$arg" == "$errfile" ]]; then
      [[ -f "$arg" && "$arg" == "$TMPDIR"/locus-hub-smoke-err.* ]] || return 90
      grep -q '^synthetic pin diagnostic$' "$arg" || return 91
      printf '%s' observed > "$ERR_OBSERVED"
    fi
  done
  command rm "$@"
}
`);
  const result = spawnSync('bash', [script], {
    env: { ...process.env, TMPDIR: scratch, BASH_ENV: hook,
      ERR_RECORD: record, ERR_OBSERVED: observed },
    encoding: 'utf8',
    timeout: 15_000,
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 0, 'smoke must complete successfully');
  assert.ok(result.stdout.split('\n').includes('== hub-smoke OK =='), 'success banner missing');
  assert.doesNotMatch(result.stdout, /^skip\s/m, 'all smoke checks must run');
  const errorFile = readFileSync(record, 'utf8').trim();
  assert.equal(dirname(errorFile), scratch);
  assert.ok(errorFile.slice(scratch.length + 1).startsWith('locus-hub-smoke-err.'));
  assert.equal(readFileSync(observed, 'utf8'), 'observed',
    'cleanup must remove the error file holding the last attempt\'s stderr');
  assert.equal(existsSync(errorFile), false, 'temporary error file must be removed');
  assert.deepEqual(readdirSync(scratch), ['locus-hub-smoke-err.unrelated']);
  assert.equal(readFileSync(join(scratch, 'locus-hub-smoke-err.unrelated'), 'utf8'), 'keep');
});

test('normal completion truncates and removes the reused error file after a retry', (t) => {
  const root = mkdtempSync('/tmp/locus-err-test-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'reused error files');
  const record = join(root, 'error-file');
  const observed = join(root, 'observed');
  const attempts = join(root, 'pin-attempts');
  const hook = join(root, 'hook.sh');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'locus-hub-smoke-err.unrelated'), 'keep');
  // The first pin attempt fails after writing a diagnostic; the retry
  // truncates the same error file, so cleanup sees only the second
  // attempt's stderr before removing it.
  writeFileSync(hook, `mktemp() {
  local path
  path="$(command mktemp "$@")" || return
  case "$path" in
    "$TMPDIR"/locus-hub-smoke-err.*) printf '%s\\n' "$path" > "$ERR_RECORD" ;;
  esac
  printf '%s\\n' "$path"
}
jq() { printf '%s\\n' grant; }
cargo() { return 0; }
sleep() { :; }
locus() {
  if [[ "\${1:-} \${2:-}" == 'status --oneline' ]]; then
    printf '%s\\n' personal:tenant
    return 0
  fi
  case "\${1:-}" in
    init)
      [[ -f "$SMOKE_ERR" && ! -s "$SMOKE_ERR" ]] || return 90
      printf 'synthetic init diagnostic\\n' >&2 ;;
    pin)
      [[ -f "$SMOKE_ERR" && ! -s "$SMOKE_ERR" \\
        && "$SMOKE_ERR" == "$TMPDIR"/locus-hub-smoke-err.* ]] || return 90
      printf 'attempt\\n' >> "$PIN_ATTEMPTS"
      local n
      n="$(wc -l < "$PIN_ATTEMPTS" | tr -d ' ')"
      printf 'pin diagnostic %s\\n' "$n" >&2
      (( n > 1 )) || return 1 ;;
  esac
  printf '%s\\n' '{}'
}
rm() {
  local errfile arg
  errfile="$(cat "$ERR_RECORD")"
  for arg in "$@"; do
    if [[ "$arg" == "$errfile" ]]; then
      [[ -f "$arg" && "$arg" == "$TMPDIR"/locus-hub-smoke-err.* ]] || return 90
      grep -q '^pin diagnostic 2$' "$arg" || return 91
      grep -q 'pin diagnostic 1' "$arg" && return 92
      printf '%s' observed > "$ERR_OBSERVED"
    fi
  done
  command rm "$@"
}
`);
  const result = spawnSync('bash', [script], {
    env: { ...process.env, TMPDIR: scratch, BASH_ENV: hook, PIN_ATTEMPTS: attempts,
      ERR_RECORD: record, ERR_OBSERVED: observed },
    encoding: 'utf8',
    timeout: 15_000,
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 0, 'smoke must complete successfully');
  assert.ok(result.stdout.split('\n').includes('== hub-smoke OK =='), 'success banner missing');
  assert.doesNotMatch(result.stdout, /^skip\s/m, 'all smoke checks must run');
  assert.match(result.stderr, /warn: hub-smoke init\/pin attempt 1 failed; retrying/);
  assert.match(result.stderr, /pin diagnostic 1/);
  assert.equal(readFileSync(attempts, 'utf8'), 'attempt\nattempt\n');
  const errorFile = readFileSync(record, 'utf8').trim();
  assert.equal(dirname(errorFile), scratch);
  assert.ok(errorFile.slice(scratch.length + 1).startsWith('locus-hub-smoke-err.'));
  assert.equal(readFileSync(observed, 'utf8'), 'observed',
    'cleanup must remove the error file truncated for the successful retry');
  assert.equal(existsSync(errorFile), false, 'temporary error file must be removed');
  assert.deepEqual(readdirSync(scratch), ['locus-hub-smoke-err.unrelated']);
  assert.equal(readFileSync(join(scratch, 'locus-hub-smoke-err.unrelated'), 'utf8'), 'keep');
});

test('normal completion leaves no error file behind with the real rm', (t) => {
  const root = mkdtempSync('/tmp/locus-err-test-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'real rm');
  const hook = join(root, 'hook.sh');
  mkdirSync(scratch);
  // Unlike the tests above, rm is not shimmed: the EXIT trap's own rm must
  // delete the populated error file (and the other temporaries) by itself.
  writeFileSync(hook, `jq() { printf '%s\\n' grant; }
cargo() { return 0; }
locus() {
  if [[ "\${1:-} \${2:-}" == 'status --oneline' ]]; then
    printf '%s\\n' personal:tenant
    return 0
  fi
  case "\${1:-}" in
    init|pin) printf 'synthetic %s diagnostic\\n' "$1" >&2 ;;
  esac
  printf '%s\\n' '{}'
}
`);
  const result = spawnSync('bash', [script], {
    env: { ...process.env, TMPDIR: scratch, BASH_ENV: hook },
    encoding: 'utf8',
    timeout: 15_000,
  });

  assert.equal(result.error, undefined);
  assert.equal(result.status, 0, 'smoke must complete successfully');
  assert.ok(result.stdout.split('\n').includes('== hub-smoke OK =='), 'success banner missing');
  assert.deepEqual(readdirSync(scratch), [], 'no temporary file or directory may remain');
});
