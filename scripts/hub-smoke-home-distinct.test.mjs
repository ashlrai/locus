// Run: node --test scripts/hub-smoke-home-distinct.test.mjs
// Every hub-smoke invocation must allocate its own SMOKE_HOME through
// mktemp -d. A fixed or recycled path would let one run's throwaway
// LOCUS_HOME observe another run's bindings and seal state — and cleanup
// of one run could delete the other's live home. These tests invoke the
// script back-to-back and concurrently under one TMPDIR and assert each
// run records a distinct home directory that it alone uses and removes.
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { basename, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-smoke.sh');

// The mktemp shim verifies, the instant the real mktemp returns, that a
// non-symlink directory exists at the returned path, and records it. The
// locus init stub then re-checks $SMOKE_HOME at first use: it must be the
// recorded directory, still a direct child of TMPDIR, exported as
// LOCUS_HOME, and — when HOME_SOLE is set — the only locus-hub-smoke.*
// entry there. When PRIOR_HOME names a previous run's directory, that
// directory must already be gone.
const hook = `mktemp() {
  local path
  path="$(command mktemp "$@")" || return
  if [[ "$path" == "$TMPDIR"/locus-hub-smoke.* ]]; then
    [[ -d "$path" && ! -L "$path" ]] || return 90
    printf '%s\\n' "$path" > "$HOME_CREATED"
  fi
  printf '%s\\n' "$path"
}
locus() {
  if [[ "\${1:-}" == init ]]; then
    [[ -d "$SMOKE_HOME" && ! -L "$SMOKE_HOME" ]] || return 91
    [[ "$SMOKE_HOME" == "$TMPDIR"/locus-hub-smoke.* ]] || return 91
    [[ "$LOCUS_HOME" == "$SMOKE_HOME" ]] || return 91
    [[ "$SMOKE_HOME" == "$(cat "$HOME_CREATED")" ]] || return 92
    if [[ -n "\${HOME_SOLE:-}" ]]; then
      local entries=("$TMPDIR"/locus-hub-smoke.*)
      [[ \${#entries[@]} -eq 1 && "\${entries[0]}" == "$SMOKE_HOME" ]] || return 93
    fi
    if [[ -n "\${PRIOR_HOME:-}" ]]; then
      [[ ! -e "$PRIOR_HOME" ]] || return 94
    fi
    printf '%s\\n' "$SMOKE_HOME" > "$HOME_USED"
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

function assertHomeDir(path, scratch) {
  assert.equal(dirname(path), scratch, 'home must be a direct child of TMPDIR');
  assert.match(basename(path), /^locus-hub-smoke\.[A-Za-z0-9]{6}$/);
  assert.equal(existsSync(path), false, 'temporary home must be removed');
}

test('back-to-back invocations create and use distinct home directories', (t) => {
  const root = mkdtempSync('/tmp/lhs-home-distinct-');
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
      HOME_CREATED: join(root, `created-${run}`), HOME_USED: join(root, `used-${run}`),
      HOME_SOLE: '1',
    };
    // The second run must not find the first run's home lingering.
    if (run === 1) env.PRIOR_HOME = created[0];
    const result = spawnSync('bash', [script], { env, encoding: 'utf8', timeout: 15_000 });

    assert.equal(result.error, undefined);
    assert.equal(result.signal, null);
    assert.equal(result.status, 0, `run ${run} stderr:\n${result.stderr}`);
    assert.match(result.stdout, /^== hub-smoke OK ==$/m);
    assert.doesNotMatch(result.stdout, /^skip\s/m, 'all smoke checks must run');
    created.push(readFileSync(env.HOME_CREATED, 'utf8').trim());
    used.push(readFileSync(env.HOME_USED, 'utf8').trim());
    assert.deepEqual(readdirSync(scratch), [], `run ${run} must leave TMPDIR empty`);
  }

  assert.notEqual(created[0], created[1], 'separate invocations must create distinct directories');
  assert.deepEqual(used, created, 'each run must use the home directory it created');
  for (const path of created) assertHomeDir(path, scratch);
});

test('concurrent invocations each get a distinct home directory', { timeout: 30_000 }, async (t) => {
  const root = mkdtempSync('/tmp/lhs-home-distinct-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'scratch dir');
  const hookPath = join(root, 'hook.sh');
  mkdirSync(scratch);
  writeFileSync(hookPath, hook);

  const run = (index) => new Promise((resolve, reject) => {
    const child = spawn('bash', [script], {
      env: {
        ...process.env, BASH_ENV: hookPath, TMPDIR: scratch,
        HOME_CREATED: join(root, `created-${index}`), HOME_USED: join(root, `used-${index}`),
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
    // Each run's init saw exactly the directory its own mktemp call created.
    assert.equal(readFileSync(join(root, `used-${result.index}`), 'utf8').trim(), path);
  }
  assert.notEqual(created[0], created[1], 'concurrent invocations must create distinct directories');
  for (const path of created) assertHomeDir(path, scratch);
  assert.deepEqual(readdirSync(scratch), [], 'both runs must leave TMPDIR empty');
});
