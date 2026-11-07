// Run: node --test scripts/hub-smoke-err-distinct.test.mjs
// Every hub-smoke invocation must allocate its own SMOKE_ERR through mktemp.
// A fixed or recycled path would let one run read another run's captured
// init/pin diagnostics — and two overlapping runs would clobber each other.
// These tests invoke the script back-to-back, concurrently and beside the
// populated error file a killed predecessor left behind (SIGKILL can orphan
// the allocated paths — see hub-smoke-interruption.test.mjs). Each run must
// record a distinct error file that it alone uses, rewrites and removes; a
// stale sibling is never adopted, drained or cleaned. (Within a run the file
// is intentionally reused across pin retries — covered by
// hub-smoke-error-failure.test.mjs.)
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { basename, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-smoke.sh');

// The mktemp shim verifies, the instant the real mktemp returns, that a
// regular empty non-symlink file exists at the returned path, and records
// it. The locus init stub then re-checks $SMOKE_ERR at first use: it must be
// the recorded file, still empty, a direct child of TMPDIR, and — when
// ERR_SOLE is set — the only locus-hub-smoke-err.* entry there. When
// PRIOR_ERR names a previous run's file, that file must already be gone.
// When STALE_ERR names a killed run's leftover, that file must still exist
// populated beside exactly one sibling: this run's fresh file.
const hook = `mktemp() {
  local path
  path="$(command mktemp "$@")" || return
  if [[ "$path" == "$TMPDIR"/locus-hub-smoke-err.* ]]; then
    [[ -f "$path" && ! -L "$path" && ! -s "$path" ]] || return 90
    printf '%s\\n' "$path" > "$ERR_CREATED"
  fi
  printf '%s\\n' "$path"
}
locus() {
  if [[ "\${1:-}" == init ]]; then
    [[ -f "$SMOKE_ERR" && ! -L "$SMOKE_ERR" && ! -s "$SMOKE_ERR" ]] || return 91
    [[ "$SMOKE_ERR" == "$TMPDIR"/locus-hub-smoke-err.* ]] || return 91
    [[ "$SMOKE_ERR" == "$(cat "$ERR_CREATED")" ]] || return 92
    if [[ -n "\${ERR_SOLE:-}" ]]; then
      local entries=("$TMPDIR"/locus-hub-smoke-err.*)
      [[ \${#entries[@]} -eq 1 && "\${entries[0]}" == "$SMOKE_ERR" ]] || return 93
    fi
    if [[ -n "\${PRIOR_ERR:-}" ]]; then
      [[ ! -e "$PRIOR_ERR" ]] || return 94
    fi
    if [[ -n "\${STALE_ERR:-}" ]]; then
      [[ -f "$STALE_ERR" && "$SMOKE_ERR" != "$STALE_ERR" ]] || return 96
      grep -q . "$STALE_ERR" || return 97
      local siblings=("$TMPDIR"/locus-hub-smoke-err.*)
      [[ \${#siblings[@]} -eq 2 ]] || return 98
    fi
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
`;

function assertErrorFile(path, scratch) {
  assert.equal(dirname(path), scratch, 'error file must be a direct child of TMPDIR');
  assert.match(basename(path), /^locus-hub-smoke-err\.[A-Za-z0-9]{6}$/);
  assert.equal(existsSync(path), false, 'temporary error file must be removed');
}

test('back-to-back invocations create and use distinct error files', (t) => {
  const root = mkdtempSync('/tmp/lhs-err-distinct-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'scratch dir');
  const hookPath = join(root, 'hook.sh');
  mkdirSync(scratch);
  writeFileSync(hookPath, hook);

  const created = [];
  const used = [];
  for (let run = 0; run < 2; run++) {
    const env = {
      ...process.env, BASH_ENV: hookPath, TMPDIR: scratch,
      ERR_CREATED: join(root, `created-${run}`), ERR_USED: join(root, `used-${run}`),
      ERR_SOLE: '1',
    };
    // The second run must not find the first run's error file lingering.
    if (run === 1) env.PRIOR_ERR = created[0];
    const result = spawnSync('bash', [script], { env, encoding: 'utf8', timeout: 15_000 });

    assert.equal(result.error, undefined);
    assert.equal(result.signal, null);
    assert.equal(result.status, 0, `run ${run} stderr:\n${result.stderr}`);
    assert.match(result.stdout, /^== hub-smoke OK ==$/m);
    assert.doesNotMatch(result.stdout, /^skip\s/m, 'all smoke checks must run');
    created.push(readFileSync(env.ERR_CREATED, 'utf8').trim());
    used.push(readFileSync(env.ERR_USED, 'utf8').trim());
    assert.deepEqual(readdirSync(scratch), [], `run ${run} must leave TMPDIR empty`);
  }

  assert.notEqual(created[0], created[1], 'separate invocations must create distinct files');
  assert.deepEqual(used, created, 'each run must use the error file it created');
  for (const path of created) assertErrorFile(path, scratch);
});

test('concurrent invocations each get a distinct error file', { timeout: 30_000 }, async (t) => {
  const root = mkdtempSync('/tmp/lhs-err-distinct-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'scratch dir');
  const hookPath = join(root, 'hook.sh');
  mkdirSync(scratch);
  writeFileSync(hookPath, hook);

  const run = (index) => new Promise((resolve, reject) => {
    const child = spawn('bash', [script], {
      env: {
        ...process.env, BASH_ENV: hookPath, TMPDIR: scratch,
        ERR_CREATED: join(root, `created-${index}`), ERR_USED: join(root, `used-${index}`),
      },
    });
    t.after(() => { if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL'); });
    let stdout = '';
    let stderr = '';
    child.stdout.on('data', (chunk) => { stdout += chunk; });
    child.stderr.on('data', (chunk) => { stderr += chunk; });
    child.once('error', reject);
    child.once('exit', (code, signal) => resolve({ code, signal, stdout, stderr, index }));
  });

  const runs = await Promise.all([run(0), run(1)]);
  const created = [];
  for (const result of runs) {
    assert.equal(result.signal, null);
    assert.equal(result.code, 0, `run ${result.index} stderr:\n${result.stderr}`);
    assert.match(result.stdout, /^== hub-smoke OK ==$/m);
    assert.doesNotMatch(result.stdout, /^skip\s/m, 'all smoke checks must run');
    const path = readFileSync(join(root, `created-${result.index}`), 'utf8').trim();
    created.push(path);
    // Each run's init saw exactly the file its own mktemp call created.
    assert.equal(readFileSync(join(root, `used-${result.index}`), 'utf8').trim(), path);
  }
  assert.notEqual(created[0], created[1], 'concurrent invocations must create distinct files');
  for (const path of created) assertErrorFile(path, scratch);
  assert.deepEqual(readdirSync(scratch), [], 'both runs must leave TMPDIR empty');
});

test('a repeated invocation beside a killed predecessor\'s error file allocates a distinct file', (t) => {
  const root = mkdtempSync('/tmp/lhs-err-distinct-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'scratch dir');
  const hookPath = join(root, 'hook.sh');
  mkdirSync(scratch);
  // A populated template-shaped leftover: what SIGKILL leaves in a shared
  // TMPDIR (hub-smoke-interruption.test.mjs). Its name occupies the same
  // suffix space mktemp draws from, so creation must skip it.
  const stale = join(scratch, 'locus-hub-smoke-err.stale0');
  writeFileSync(stale, 'stale predecessor diagnostic\n');
  writeFileSync(hookPath, hook);

  const env = {
    ...process.env, BASH_ENV: hookPath, TMPDIR: scratch,
    ERR_CREATED: join(root, 'created'), ERR_USED: join(root, 'used'), STALE_ERR: stale,
  };
  const result = spawnSync('bash', [script], { env, encoding: 'utf8', timeout: 15_000 });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 0, `stderr:\n${result.stderr}`);
  assert.match(result.stdout, /^== hub-smoke OK ==$/m);
  assert.doesNotMatch(result.stdout, /^skip\s/m, 'all smoke checks must run');
  const fresh = readFileSync(env.ERR_CREATED, 'utf8').trim();
  assert.notEqual(fresh, stale, 'mktemp must not hand back the leftover path');
  assert.equal(readFileSync(env.ERR_USED, 'utf8').trim(), fresh, 'the run must use the file it created');
  assertErrorFile(fresh, scratch);
  assert.equal(readFileSync(stale, 'utf8'), 'stale predecessor diagnostic\n',
    'the stale file must survive untouched — never adopted, drained or removed');
  assert.deepEqual(readdirSync(scratch), [basename(stale)], 'cleanup removes only the run\'s own file');
});

test('pin retries rewrite only the fresh file beside a stale leftover', (t) => {
  const root = mkdtempSync('/tmp/lhs-err-distinct-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'scratch dir');
  const hookPath = join(root, 'hook.sh');
  mkdirSync(scratch);
  const stale = join(scratch, 'locus-hub-smoke-err.old123');
  writeFileSync(stale, 'stale predecessor diagnostic\n');
  const created = join(root, 'created');
  const record = join(root, 'used-paths');
  // pin fails all three attempts. Every retry's head dump and the final
  // un-silenced pin must surface only this run's diagnostics: truncation or
  // redirection aimed at the stale file would leak its bytes into stderr or
  // leave it emptied.
  writeFileSync(hookPath, `mktemp() {
  local path
  path="$(command mktemp "$@")" || return
  if [[ "$path" == "$TMPDIR"/locus-hub-smoke-err.* ]]; then
    [[ -f "$path" && ! -L "$path" && ! -s "$path" && "$path" != "$STALE_ERR" ]] || return 90
    printf '%s\\n' "$path" > "$ERR_CREATED"
  fi
  printf '%s\\n' "$path"
}
locus() {
  case "\${1:-}" in
    init)
      [[ "$SMOKE_ERR" == "$(cat "$ERR_CREATED")" && "$SMOKE_ERR" != "$STALE_ERR" ]] || return 91
      printf '%s\\n' "$SMOKE_ERR" >> "$ERR_USED" ;;
    pin) printf 'fresh pin diagnostic\\n' >&2; return 1 ;;
  esac
  if [[ "\${1:-} \${2:-}" == 'status --oneline' ]]; then
    printf '%s\\n' personal:tenant
  else
    printf '%s\\n' '{}'
  fi
}
sleep() { :; }
jq() { cat >/dev/null; printf '%s\\n' grant; }
cargo() { return 0; }
`);
  const result = spawnSync('bash', [script], {
    env: { ...process.env, BASH_ENV: hookPath, TMPDIR: scratch,
      ERR_CREATED: created, ERR_USED: record, STALE_ERR: stale },
    encoding: 'utf8', timeout: 15_000,
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /warn: hub-smoke init\/pin attempt 1 failed; retrying/);
  assert.match(result.stderr, /warn: hub-smoke init\/pin attempt 2 failed; retrying/);
  assert.match(result.stderr, /error: hub-smoke init\/pin failed after 3 attempts/);
  assert.match(result.stderr, /fresh pin diagnostic/);
  assert.doesNotMatch(result.stderr, /stale predecessor diagnostic/,
    'retry diagnostics must come only from this run\'s error file');
  const fresh = readFileSync(created, 'utf8').trim();
  assert.notEqual(fresh, stale, 'mktemp must not hand back the leftover path');
  // init ran once per attempt, always against the same fresh file.
  assert.deepEqual([...new Set(readFileSync(record, 'utf8').trim().split('\n'))], [fresh]);
  assert.equal(existsSync(fresh), false, 'cleanup removes the used file even on failure');
  assert.equal(readFileSync(stale, 'utf8'), 'stale predecessor diagnostic\n');
  assert.deepEqual(readdirSync(scratch), [basename(stale)]);
});
