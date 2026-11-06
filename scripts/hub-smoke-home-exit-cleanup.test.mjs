// Run: node --test scripts/hub-smoke-home-exit-cleanup.test.mjs
// On a successful exit the script's EXIT trap must delete SMOKE_HOME while
// it still holds the throwaway LOCUS_HOME state. These fixtures stub every
// external command: the mktemp shim records each allocation, the locus
// stub populates LOCUS_HOME at init, and the rm shim verifies the home is
// still populated at removal time — so a cleanup that emptied, skipped, or
// mistargeted the directory would be caught. Removal itself uses the
// script's own rm -rf; the shim only observes.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { basename, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-smoke.sh');

// Every command the script invokes is stubbed so the run reaches the
// success banner without a build or broker. The pin stub fails once when
// PIN_FAIL_ONCE is set, driving the retry path to the same successful exit.
const shims = `mktemp() {
  local path kind
  path="$(command mktemp "$@")" || return
  if [[ -d "$path" ]]; then kind=directory; else kind=file; fi
  printf '%s\\t%s\\n' "$kind" "$path" >> "$ALLOCATIONS"
  printf '%s\\n' "$path"
}
jq() { cat >/dev/null; printf '%s\\n' grant; }
cargo() { return 0; }
sleep() { :; }
locus() {
  if [[ "\${1:-} \${2:-}" == 'status --oneline' ]]; then
    printf '%s\\n' personal:tenant
    return 0
  fi
  case "\${1:-}" in
    init)
      [[ -d "$LOCUS_HOME" && "$LOCUS_HOME" == "$TMPDIR"/locus-hub-smoke.* ]] || return 90
      printf '%s' "$LOCUS_HOME" > "$HOME_RECORD"
      mkdir -p "$LOCUS_HOME/bindings/nested dir" "$LOCUS_HOME/sessions" || return 91
      printf 'binding\\n' > "$LOCUS_HOME/bindings/personal.toml" || return 91
      printf 'key\\n' > "$LOCUS_HOME/daemon.key" || return 91
      printf 'pin\\n' > "$LOCUS_HOME/sessions/active.session" || return 91
      printf 'payload\\n' > "$LOCUS_HOME/bindings/nested dir/file with spaces.txt" || return 91
      # The retried init must rewrite the read-only fixture it left behind.
      [[ ! -f "$LOCUS_HOME/readonly.seal" ]] || chmod 0644 "$LOCUS_HOME/readonly.seal" || return 91
      printf 'sealed\\n' > "$LOCUS_HOME/readonly.seal" || return 91
      chmod 0444 "$LOCUS_HOME/readonly.seal" || return 91 ;;
    pin)
      printf 'attempt\\n' >> "$PIN_ATTEMPTS"
      if [[ -n "\${PIN_FAIL_ONCE:-}" && ! -f "$PIN_FAILED" ]]; then
        : > "$PIN_FAILED"
        printf 'synthetic first pin failure\\n' >&2
        return 1
      fi ;;
  esac
  printf '%s\\n' '{}'
}
rm() {
  printf '%s\\n' "$*" >> "$RM_CALLS"
  local home arg targeted=
  if [[ -f "$HOME_RECORD" ]]; then
    home="$(cat "$HOME_RECORD")"
    for arg in "$@"; do
      [[ "$arg" == "$home" ]] && targeted=1
    done
  fi
  if [[ -n "$targeted" ]]; then
    if [[ -f "$home/bindings/personal.toml" && -f "$home/daemon.key" \\
      && -f "$home/sessions/active.session" \\
      && -f "$home/bindings/nested dir/file with spaces.txt" \\
      && -f "$home/readonly.seal" ]]; then
      printf 'populated' > "$HOME_OBSERVED"
    else
      printf 'unpopulated' > "$HOME_OBSERVED"
    fi
  fi
  command rm "$@"
}
`;

function fixture(t) {
  // Keep the path short enough for the authority broker's Unix socket.
  const root = mkdtempSync('/tmp/lhs-exit-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'scratch');
  const hook = join(root, 'hook.sh');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'sentinel');
  writeFileSync(hook, shims);
  const files = {
    allocations: join(root, 'allocations'),
    homeRecord: join(root, 'home'),
    observed: join(root, 'observed'),
    rmCalls: join(root, 'rm-calls'),
    pinAttempts: join(root, 'pin-attempts'),
    pinFailed: join(root, 'pin-failed'),
  };
  const env = {
    ...process.env, TMPDIR: scratch, BASH_ENV: hook,
    ALLOCATIONS: files.allocations, HOME_RECORD: files.homeRecord,
    HOME_OBSERVED: files.observed, RM_CALLS: files.rmCalls,
    PIN_ATTEMPTS: files.pinAttempts, PIN_FAILED: files.pinFailed,
  };
  return { scratch, files, env };
}

function assertSuccess(result) {
  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 0, `smoke must complete successfully:\n${result.stderr}`);
  assert.ok(result.stdout.split('\n').includes('== hub-smoke OK =='), 'success banner missing');
  assert.doesNotMatch(result.stdout, /^skip\s/m, 'all smoke checks must run');
}

// The mktemp shim records allocations in script order: home dir, project
// dir, error file. The returned home path is what locus init exported.
function readAllocations(files) {
  const allocations = readFileSync(files.allocations, 'utf8').trim().split('\n').map((line) => line.split('\t'));
  assert.equal(allocations.length, 3);
  const expected = [['directory', 'locus-hub-smoke.'], ['directory', 'locus-hub-proj.'], ['file', 'locus-hub-smoke-err.']];
  for (const [index, [kind, prefix]] of expected.entries()) {
    assert.equal(allocations[index][0], kind);
    assert.ok(basename(allocations[index][1]).startsWith(prefix), `${prefix} allocation ${index}`);
  }
  return allocations;
}

test('successful exit removes the populated SMOKE_HOME in one rm -rf call', (t) => {
  const { scratch, files, env } = fixture(t);
  const result = spawnSync('bash', [script], { env, encoding: 'utf8', timeout: 15_000 });

  assertSuccess(result);
  const home = readFileSync(files.homeRecord, 'utf8');
  assert.equal(dirname(home), scratch);
  assert.ok(basename(home).startsWith('locus-hub-smoke.'));
  assert.ok(result.stdout.includes(`== hub-smoke LOCUS_HOME=${home} ==`),
    'the removed directory must be the exported LOCUS_HOME');
  const allocations = readAllocations(files);
  assert.equal(allocations[0][1], home, 'init must use the directory mktemp created');
  assert.equal(readFileSync(files.observed, 'utf8'), 'populated',
    'cleanup must remove the home while its LOCUS_HOME state is still inside');
  const rmCalls = readFileSync(files.rmCalls, 'utf8').trim().split('\n');
  assert.deepEqual(rmCalls, [`-rf -- ${home} ${allocations[1][1]} ${allocations[2][1]}`],
    'cleanup must be the only rm invocation: one rm -rf -- over all three resources');
  for (const [, path] of allocations) {
    assert.equal(existsSync(path), false, `${path} must be removed`);
  }
  assert.deepEqual(readdirSync(scratch), ['keep']);
  assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'sentinel');
});

test('successful exit after a pin retry still removes the populated SMOKE_HOME', (t) => {
  const { scratch, files, env } = fixture(t);
  const result = spawnSync('bash', [script], {
    env: { ...env, PIN_FAIL_ONCE: '1' }, encoding: 'utf8', timeout: 15_000,
  });

  assertSuccess(result);
  assert.match(result.stderr, /warn: hub-smoke init\/pin attempt 1 failed; retrying/);
  assert.match(result.stderr, /synthetic first pin failure/);
  assert.equal(readFileSync(files.pinAttempts, 'utf8'), 'attempt\nattempt\n');
  const home = readFileSync(files.homeRecord, 'utf8');
  assert.equal(dirname(home), scratch);
  assert.ok(basename(home).startsWith('locus-hub-smoke.'));
  const allocations = readAllocations(files);
  assert.equal(allocations[0][1], home);
  assert.equal(readFileSync(files.observed, 'utf8'), 'populated',
    'cleanup must remove the home while its LOCUS_HOME state is still inside');
  assert.deepEqual(readFileSync(files.rmCalls, 'utf8').trim().split('\n'),
    [`-rf -- ${home} ${allocations[1][1]} ${allocations[2][1]}`]);
  for (const [, path] of allocations) {
    assert.equal(existsSync(path), false, `${path} must be removed`);
  }
  assert.deepEqual(readdirSync(scratch), ['keep']);
  assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'sentinel');
});
