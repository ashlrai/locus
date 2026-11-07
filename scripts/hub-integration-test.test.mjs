// Run: node --test scripts/hub-integration-test.test.mjs
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import net from 'node:net';
import { basename, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-integration-test.sh');
const realMktemp = execFileSync('which', ['mktemp'], { encoding: 'utf8' }).trim();
const realRm = execFileSync('which', ['rm'], { encoding: 'utf8' }).trim();
const realCurl = execFileSync('which', ['curl'], { encoding: 'utf8' }).trim();
const realFind = execFileSync('which', ['find'], { encoding: 'utf8' }).trim();

test('the report hygiene comment points to the filesystem canary test', () => {
  const source = readFileSync(script, 'utf8');
  const reference = source.match(/filesystem canary scan in (tests\/integration\/phantom_locus\.rs)/);
  assert.ok(reference, 'document the separate filesystem canary scan');
  const target = join(dirname(script), '..', reference[1]);
  assert.ok(existsSync(target), 'the documented canary test must exist');
  assert.match(readFileSync(target, 'utf8'), /fn canary_scan_unreadable_path_exits_nonzero\(/);
});

function runWithTempDir(t, failProject = false, scratchParts = ['custom-tmp']) {
  // Keep paths short enough for the live authority broker's Unix socket.
  const root = mkdtempSync('/tmp/lhi-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const bin = join(root, 'bin');
  const scratch = join(root, ...scratchParts);
  const record = join(root, 'created');
  const cleanupRecord = join(root, 'cleanup-observed');
  const homeRecord = join(root, 'initialized-home-observed');
  const projectRecord = join(root, 'project-observed');
  const projectAttempt = join(root, 'project-attempt');
  const scanRecord = join(root, 'scan-listings');
  mkdirSync(bin);
  mkdirSync(scratch, { recursive: true });
  writeFileSync(join(scratch, 'keep'), 'sentinel');
  writeFileSync(join(scratch, '.locus-hub-int-scan.unrelated'), 'hidden sentinel');
  writeFileSync(join(bin, 'mktemp'), `#!/usr/bin/env bash
if [[ "${'$'}{FAIL_PROJECT:-}" == 1 && "${'$'}{*: -1}" == */locus-hub-int-proj.XXXXXX ]]; then
  [[ -f "$(cat "${'$'}MKTEMP_RECORD")/home-sentinel" ]] || exit 74
  : > "${'$'}PROJECT_ATTEMPT"
  exit 73
fi
path="$("${'$'}REAL_MKTEMP" "${'$'}@")" || exit
[[ -d "${'$'}path" ]] || exit 1
touch "${'$'}path/cleanup-sentinel" || exit 1
if [[ "${'$'}{FAIL_PROJECT:-}" == 1 ]]; then : > "${'$'}path/home-sentinel"; fi
printf '%s\\n' "${'$'}path" >> "${'$'}MKTEMP_RECORD"
printf '%s\\n' "${'$'}path"
`, { mode: 0o755 });
  writeFileSync(join(bin, 'rm'), `#!/usr/bin/env bash
if [[ -f "${'$'}MKTEMP_RECORD" && ! -f "${'$'}CLEANUP_RECORD" ]]; then
  for arg in "${'$'}@"; do
    case "${'$'}arg" in "${'$'}TMPDIR"/locus-hub-int.*|"${'$'}TMPDIR"/locus-hub-int-proj.*) ;; *) continue ;; esac
    home_dir="$(sed -n '1p' "${'$'}MKTEMP_RECORD")"
    project_dir="$(sed -n '2p' "${'$'}MKTEMP_RECORD")"
    if [[ -n "${'$'}project_dir" && ( "${'$'}arg" == "${'$'}home_dir" || "${'$'}arg" == "${'$'}project_dir" ) ]]; then
      [[ -f "${'$'}home_dir/cleanup-sentinel" && -f "${'$'}project_dir/cleanup-sentinel" ]] || exit 75
      if [[ "${'$'}LOCUS_HOME" == "${'$'}home_dir" && -f "${'$'}home_dir/bindings/personal.toml" && -f "${'$'}home_dir/daemon.key" && -f "${'$'}home_dir/sessions/active.json" ]]; then
        printf '%s\\n' "${'$'}LOCUS_HOME" > "${'$'}HOME_RECORD"
      fi
      if [[ "${'$'}PWD" -ef "${'$'}project_dir" && -f "${'$'}project_dir/.mcp.json" && -f "${'$'}project_dir/hub-merged.mcp.json" ]]; then
        : > "${'$'}PROJECT_RECORD"
      fi
      : > "${'$'}CLEANUP_RECORD"
      break
    fi
  done
fi
exec "${'$'}REAL_RM" "${'$'}@"
`, { mode: 0o755 });
  // The shell opens the scan listing before find starts, so observe the real
  // file while it exists instead of inferring its creation from script text.
  writeFileSync(join(bin, 'find'), `#!/usr/bin/env bash
if [[ "${'$'}#" == 4 && "${'$'}2" == -type && "${'$'}3" == f && "${'$'}4" == -print0 ]]; then
  shopt -s nullglob
  listings=("${'$'}TMPDIR"/.locus-hub-int-scan.[0-9]*.*)
  [[ "${'$'}{#listings[@]}" == 1 && -f "${'$'}{listings[0]}" ]] || exit 76
  printf '%s\\n' "${'$'}{listings[0]}" >> "${'$'}SCAN_RECORD"
fi
exec "${'$'}REAL_FIND" "${'$'}@"
`, { mode: 0o755 });
  const result = spawnSync('bash', [script], {
    env: { ...process.env, TMPDIR: scratch, PATH: `${bin}:${process.env.PATH}`,
      REAL_MKTEMP: realMktemp, MKTEMP_RECORD: record, PROJECT_ATTEMPT: projectAttempt,
      FAIL_PROJECT: failProject ? '1' : '0', REAL_RM: realRm, CLEANUP_RECORD: cleanupRecord,
      PROJECT_RECORD: projectRecord, HOME_RECORD: homeRecord, REAL_FIND: realFind, SCAN_RECORD: scanRecord },
    encoding: 'utf8',
    maxBuffer: 4 * 1024 * 1024,
  });
  const paths = readFileSync(record, 'utf8').trim().split('\n');
  return { result, scratch, paths, projectAttempt, cleanupRecord, projectRecord, homeRecord, scanRecord };
}

test('successful integration smoke removes the initialized SMOKE_HOME', (t) => {
  const { result, scratch, paths, homeRecord } = runWithTempDir(t);
  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 0, 'integration smoke must succeed');
  assert.doesNotMatch(result.stdout, /^skip\s/m, 'all integration checks must run');
  assert.match(result.stdout, /^== hub-integration-test OK ==$/m);
  assert.equal(paths.length, 3, 'both temporary directories and the spaced-TMPDIR probe must be allocated');
  assert.equal(dirname(paths[2]), join(paths[1], 'tmp with spaces'),
    'the spaced-TMPDIR probe allocates beneath the project');
  assert.equal(existsSync(homeRecord), true, 'the binding, seal key, and active session must exist at cleanup');
  const home = readFileSync(homeRecord, 'utf8').trim();
  assert.equal(home, paths[0], 'LOCUS_HOME must be the allocated SMOKE_HOME');
  assert.equal(dirname(home), scratch);
  assert.ok(result.stdout.includes(`== hub-integration-test LOCUS_HOME=${home} ==`),
    'the smoke run must report the same home');
  assert.equal(existsSync(home), false, 'the initialized SMOKE_HOME must be removed');
});

test('pin failure removes the initialized temporary home and its contents', (t) => {
  const root = mkdtempSync('/tmp/lhi-pin-fail-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'scratch with spaces');
  const observed = join(root, 'initialized-home');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'sentinel');

  // Let the real CLI initialize the home, then fail at pin only after checking
  // its state and recording the path while it still exists.
  const harness = `
locus() {
  if [[ "$1" == pin ]]; then
    [[ -f "$LOCUS_HOME/bindings/personal.toml" && -f "$LOCUS_HOME/daemon.key" ]] || return 74
    mkdir -p "$LOCUS_HOME/nested/fixture" || return 75
    : > "$LOCUS_HOME/nested/fixture/sentinel"
    printf '%s\\n' "$LOCUS_HOME" > "$OBSERVED"
    return 73
  fi
  command locus "$@"
}
export -f locus
exec bash "$1"
`;
  const result = spawnSync('bash', ['-c', harness, 'pin-failure-test', script], {
    env: { ...process.env, TMPDIR: scratch, OBSERVED: observed },
    encoding: 'utf8',
    maxBuffer: 4 * 1024 * 1024,
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 73, 'the injected pin failure must propagate');
  const home = readFileSync(observed, 'utf8').trim();
  assert.equal(dirname(home), scratch);
  assert.match(basename(home), /^locus-hub-int\.[^/]+$/);
  assert.equal(existsSync(home), false, 'the initialized home and nested fixture must be removed');
  assert.deepEqual(readdirSync(scratch), ['keep'], 'cleanup must remove the project as well');
  assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'sentinel');
});

test('integration smoke uses and cleans a nondefault TMPDIR', (t) => {
  const { result, scratch, paths } = runWithTempDir(t);
  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 0, 'integration smoke must succeed');
  assert.doesNotMatch(result.stdout, /^skip\s/m, 'all integration checks must run');
  assert.match(result.stdout, /^== hub-integration-test OK ==$/m);
  assert.equal(paths.length, 3);
  assert.match(paths[0], new RegExp(`^${scratch}/locus-hub-int\\.[^/]+$`));
  assert.match(paths[1], new RegExp(`^${scratch}/locus-hub-int-proj\\.[^/]+$`));
  assert.equal(dirname(paths[2]), join(paths[1], 'tmp with spaces'),
    'the spaced-TMPDIR probe allocates beneath the project');
  for (const path of paths) {
    assert.equal(existsSync(path), false, `populated temporary directory must be removed: ${path}`);
  }
  assert.deepEqual(readdirSync(scratch).sort(), ['.locus-hub-int-scan.unrelated', 'keep']);
  assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'sentinel');
  assert.equal(readFileSync(join(scratch, '.locus-hub-int-scan.unrelated'), 'utf8'), 'hidden sentinel');
});

test('normal exit removes the populated smoke project and temporary home', (t) => {
  const { result, paths, cleanupRecord, projectRecord } = runWithTempDir(t);
  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 0, 'integration smoke must finish normally');
  assert.doesNotMatch(result.stdout, /^skip\s/m, 'all integration checks must run');
  assert.match(result.stdout, /^== hub-integration-test OK ==$/m);
  assert.equal(paths.length, 3, 'both temporary directories and the spaced-TMPDIR probe must be allocated');
  assert.equal(dirname(paths[2]), join(paths[1], 'tmp with spaces'),
    'the spaced-TMPDIR probe allocates beneath the project');
  assert.equal(existsSync(cleanupRecord), true, 'exit cleanup must receive both populated directories');
  assert.equal(existsSync(projectRecord), true, 'the project must be the working directory and contain generated files at cleanup');
  assert.equal(existsSync(paths[0]), false, 'temporary home must be removed');
  assert.equal(existsSync(paths[1]), false, 'temporary project must be removed');
  assert.equal(existsSync(paths[2]), false, 'spaced-TMPDIR probe must be removed');
});

for (const [label, parts] of [
  ['spaces', ['temp directory']],
  ['spaces and shell metacharacters', ['a [b]', 'c d']],
]) test(`integration smoke uses and cleans TMPDIR with ${label}`, (t) => {
  const { result, scratch, paths, scanRecord, cleanupRecord, projectRecord, homeRecord } =
    runWithTempDir(t, false, parts);
  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 0, 'integration smoke must succeed');
  assert.doesNotMatch(result.stdout, /^skip\s/m, 'all integration checks must run');
  assert.equal(paths.length, 3);
  assert.ok(paths.slice(0, 2).every((path) => dirname(path) === scratch && path.includes(' ')));
  assert.match(paths[0].slice(scratch.length + 1), /^locus-hub-int\.[^/]+$/);
  assert.match(paths[1].slice(scratch.length + 1), /^locus-hub-int-proj\.[^/]+$/);
  assert.equal(dirname(paths[2]), join(paths[1], 'tmp with spaces'),
    'the spaced-TMPDIR probe allocates beneath the project');
  assert.ok(result.stdout.includes(`registerLocusInMcpConfig merge write (${paths[1]}/hub-merged.mcp.json)`));
  assert.match(result.stdout, /^ok\s+mt: live drop-in slice /m);
  assert.match(result.stdout, /^ok\s+SMOKE_HOME created beneath TMPDIR containing spaces$/m,
    'the spaced-TMPDIR home probe must pass during a green run');
  assert.match(result.stdout, /^ok\s+canary scan detects a canary under a spaced path$/m);
  assert.match(result.stdout, /^ok\s+ambient canary absent under /m);
  assert.match(result.stdout, /^== hub-integration-test OK ==$/m);
  assert.equal(existsSync(cleanupRecord), true, 'both populated directories must reach cleanup');
  assert.equal(existsSync(projectRecord), true, 'the populated project must be used before cleanup');
  assert.equal(readFileSync(homeRecord, 'utf8').trim(), paths[0],
    'the initialized home must be the directory created under TMPDIR');
  for (const path of paths) {
    assert.equal(existsSync(path), false, `populated temporary directory must be removed: ${path}`);
  }
  const listings = readFileSync(scanRecord, 'utf8').trim().split('\n');
  assert.equal(listings.length, 4, 'both canary regressions and both final scans must create listings');
  for (const listing of listings) {
    assert.equal(dirname(listing), scratch, 'each scan listing must be directly under TMPDIR');
    assert.match(basename(listing), /^\.locus-hub-int-scan\.\d+\.\d+$/);
    assert.equal(existsSync(listing), false, 'each scan listing must be removed');
  }
  assert.deepEqual(readdirSync(scratch).sort(), ['.locus-hub-int-scan.unrelated', 'keep']);
  assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'sentinel');
  assert.equal(readFileSync(join(scratch, '.locus-hub-int-scan.unrelated'), 'utf8'), 'hidden sentinel');
});

// The in-script probe allocates a home beneath "$SMOKE_PROJ/tmp with spaces"
// and must fail the run when creation does not land a directory under that
// exact root. A stub run reaches the probe with every earlier section green,
// and the shim makes make_smoke_dir either fail outright or report success
// for a directory under a word-split fragment of the spaced root.
for (const [label, mode] of [
  ['failing', 'fail'],
  ['misplaced', 'misplace'],
]) test(`a ${label} home allocation beneath a spaced TMPDIR fails the run`, (t) => {
  const root = mkdtempSync('/tmp/lhi-probe-fail-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'scratch with spaces');
  const state = join(root, 'pin-state');
  const misplaced = join(root, 'misplaced');
  mkdirSync(scratch);
  mkdirSync(state);
  writeFileSync(join(scratch, 'keep'), 'sentinel');
  writeFileSync(join(scratch, '.locus-hub-int-scan.unrelated'), 'hidden sentinel');

  const mintEnv = '"env":{"LOCUS_BINDING_ID":"bid-personal","LOCUS_BINDING":"personal","LOCUS_SESSION_ID":"ses_stub","LOCUS_TENANT":"personal","LOCUS_SEAL":"hmac-sha256:stub","LOCUS_WORKER_HOME":"/tmp/stub-worker","LOCUS_EXECUTOR_CAPABILITY":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"';
  const mint = (expires) => `{"binding":"personal","binding_id":"bid-personal","session_id":"ses_stub","tenant":"personal","seal":"hmac-sha256:stub","worker_home":"/tmp/stub-worker","expires_at":"${expires}","secrets_resolved":false,${mintEnv},"LOCUS_EXPIRES_AT":"${expires}"}}`;
  const harness = `
node() {
  if [[ -n "${'$'}{HUB_INT_MCP_OUT:-}" ]]; then
    printf '%s\\n' '{"mcpServers":{"phantom":{"command":"phantom-mcp","args":[]},"locus":{"command":"locus-mcp","args":[]}}}' > "$HUB_INT_MCP_OUT"
  fi
  [[ "${'$'}{HUB_MINT_JSON:-}" != *1970-01-01* ]]
}
cargo() { :; }
locus() {
  case "${'$'}{1:-}" in
    init) : > "$LOCUS_HOME/initialized" ;;
    pin) rm -f "$STUB_STATE/left" ;;
    leave) : > "$STUB_STATE/left" ;;
    status)
      if [[ "${'$'}{2:-}" == --oneline ]]; then
        if [[ -f "$STUB_STATE/left" ]]; then printf 'unpinned\\n'; else printf 'personal:tenant\\n'; fi
      else printf '%s\\n' '{}'; fi ;;
    agent)
      if [[ -f "$STUB_STATE/left" ]]; then
        printf '%s\\n' '{"status":"protected","ready":false,"status_oneline":"unpinned","required_servers":["locus","phantom"],"mcp_command":"locus-mcp"}'
      else
        printf '%s\\n' '{"status":"ready","ready":true,"status_oneline":"personal:tenant","required_servers":["locus","phantom"],"mcp_command":"locus-mcp"}'
      fi ;;
    ci)
      if [[ "${'$'}{2:-}" == mint ]]; then
        if [[ " $* " == *' --ttl '* ]]; then
          printf '%s\\n' '${mint('1970-01-01T00:00:00Z')}'
        else
          printf '%s\\n' '${mint('2030-01-01T00:00:00Z')}'
        fi
      else printf '%s\\n' '{}'; fi ;;
    mcp) return 1 ;;
    *) printf '%s\\n' '{}' ;;
  esac
}
mktemp() {
  if [[ "$TMPDIR" == "$SMOKE_PROJ/tmp with spaces" ]]; then
    [[ "$PROBE_MODE" == misplace ]] || return 1
    mkdir -p -- "$SMOKE_PROJ/tmp" || return
    local created
    created="$(command mktemp -d "$SMOKE_PROJ/tmp/locus-hub-int.XXXXXX")" || return
    printf '%s\\n' "$created" >> "$MISPLACED"
    printf '%s\\n' "$created"
    return
  fi
  command mktemp "$@"
}
export -f node cargo locus mktemp
exec bash "$1"
`;
  const result = spawnSync('bash', ['-c', harness, 'probe-failure-test', script], {
    env: { ...process.env, TMPDIR: scratch, STUB_STATE: state,
      PROBE_MODE: mode, MISPLACED: misplaced },
    encoding: 'utf8',
    maxBuffer: 4 * 1024 * 1024,
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 1, 'the probe failure must exit through the fail flag');
  assert.doesNotMatch(result.stdout, /^ok\s+SMOKE_HOME created beneath TMPDIR containing spaces$/m);
  assert.match(result.stdout, /^FAIL\s+SMOKE_HOME creation failed beneath TMPDIR containing spaces$/m);
  assert.match(result.stdout, /^== hub-integration-test FAILED ==$/m);
  assert.equal((result.stdout.match(/^FAIL\s/gm) || []).length, 1,
    'the spaced-TMPDIR probe must be the only failing check');
  assert.match(result.stdout, /^ok\s+ambient canary absent under /m,
    'checks after the probe must still run');

  if (mode === 'misplace') {
    const created = readFileSync(misplaced, 'utf8').trim().split('\n');
    assert.equal(created.length, 1, 'the shim must report one misplaced home');
    assert.match(created[0],
      new RegExp(`^${scratch}/locus-hub-int-proj\\.[^/]+/tmp/locus-hub-int\\.[^/]+$`),
      'the reported path must sit under the first word-split fragment, not the spaced root');
    assert.equal(existsSync(created[0]), false, 'the misplaced home must be removed with the project');
  }
  assert.deepEqual(readdirSync(scratch).sort(), ['.locus-hub-int-scan.unrelated', 'keep'],
    'cleanup must leave only the sentinels under TMPDIR');
  assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'sentinel');
  assert.equal(readFileSync(join(scratch, '.locus-hub-int-scan.unrelated'), 'utf8'), 'hidden sentinel');
});

test('failed project allocation cleans the temporary home', (t) => {
  const { result, scratch, paths, projectAttempt } = runWithTempDir(t, true);
  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 73, 'the injected project allocation failure must propagate');
  assert.equal(existsSync(projectAttempt), true, 'project allocation must be attempted');
  assert.equal(paths.length, 1);
  assert.equal(existsSync(paths[0]), false, 'the populated temporary home must be removed');
  assert.match(paths[0], new RegExp(`^${scratch}/locus-hub-int\\.[^/]+$`));
  assert.deepEqual(readdirSync(scratch).sort(), ['.locus-hub-int-scan.unrelated', 'keep']);
});

test('a controlled early failure removes both allocated temporary directories', (t) => {
  const root = mkdtempSync('/tmp/lhi-early-fail-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'scratch with spaces');
  const record = join(root, 'observed');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'sentinel');

  // Fail at the first node --test prerequisite — the earliest point where both
  // temporary directories exist but LOCUS_HOME is not yet exported, no locus
  // command has run, and the project has not been entered. The stub populates
  // both directories first so cleanup must remove non-empty trees.
  const harness = `
node() {
  [[ "$1" == --test ]] || return 74
  [[ -d "$SMOKE_HOME" && -d "$SMOKE_PROJ" && "$SMOKE_HOME" != "$SMOKE_PROJ" ]] || return 75
  [[ -z "$(ls -A "$SMOKE_HOME")" && -z "$(ls -A "$SMOKE_PROJ")" ]] || return 76
  mkdir -p "$SMOKE_HOME/nested" "$SMOKE_PROJ/nested" || return 77
  : > "$SMOKE_HOME/nested/fixture" || return 78
  : > "$SMOKE_PROJ/nested/fixture" || return 79
  printf '%s\\n' "$SMOKE_HOME" "$SMOKE_PROJ" > "$OBSERVED"
  return 73
}
jq() { :; }
cargo() { :; }
locus() { :; }
export -f node jq cargo locus
exec bash "$1"
`;
  const result = spawnSync('bash', ['-c', harness, 'early-failure-test', script], {
    env: { PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: scratch, OBSERVED: record },
    encoding: 'utf8',
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 73, 'the injected prerequisite failure must propagate');
  const paths = readFileSync(record, 'utf8').trim().split('\n');
  assert.equal(paths.length, 2, 'both temporary directories must be recorded');
  const [home, project] = paths;
  assert.equal(dirname(home), scratch);
  assert.equal(dirname(project), scratch);
  assert.match(basename(home), /^locus-hub-int\.[^/]+$/);
  assert.match(basename(project), /^locus-hub-int-proj\.[^/]+$/);
  assert.equal(existsSync(home), false, 'the populated temporary home must be removed');
  assert.equal(existsSync(project), false, 'the populated temporary project must be removed');
  assert.deepEqual(readdirSync(scratch), ['keep']);
  assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'sentinel');
});

test('real temporary directories stay under TMPDIR and are removed after init fails', (t) => {
  const root = mkdtempSync('/tmp/lhi-direct-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'custom [temp]');
  const record = join(root, 'observed');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'sentinel');

  // Observe the real mktemp results at init, before the EXIT trap removes them.
  const harness = `
node() { :; }
jq() { :; }
cargo() { :; }
locus() {
  if [[ "$1" == init ]]; then
    local projects=("$TMPDIR"/locus-hub-int-proj.*)
    [[ -d "$LOCUS_HOME" && "${'$'}{LOCUS_HOME%/*}" == "$TMPDIR" ]] || return 74
    [[ "${'$'}{#projects[@]}" == 1 && -d "${'$'}{projects[0]}" ]] || return 75
    touch "$LOCUS_HOME/fixture" "${'$'}{projects[0]}/fixture" || return 76
    printf '%s\\n' "$LOCUS_HOME" "${'$'}{projects[0]}" > "$OBSERVED"
    return 73
  fi
}
export -f node jq cargo locus
exec bash "$1"
`;
  const result = spawnSync('bash', ['-c', harness, 'cleanup-test', script], {
    env: { PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: scratch, OBSERVED: record },
    encoding: 'utf8',
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 73, 'the injected initialization failure must propagate');
  const paths = readFileSync(record, 'utf8').trim().split('\n');
  assert.equal(paths.length, 2);
  assert.match(paths[0].slice(scratch.length + 1), /^locus-hub-int\.[^/]+$/);
  assert.match(paths[1].slice(scratch.length + 1), /^locus-hub-int-proj\.[^/]+$/);
  for (const path of paths) {
    assert.equal(dirname(path), scratch, 'each directory must be directly under TMPDIR');
    assert.equal(existsSync(path), false, 'each populated directory must be removed on exit');
  }
  assert.deepEqual(readdirSync(scratch), ['keep']);
  assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'sentinel');
});

test('early initialization failure cleans both temporary directories', (t) => {
  const root = mkdtempSync('/tmp/lhi-early-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const bin = join(root, 'bin');
  const scratch = join(root, 'scratch with spaces');
  const record = join(root, 'created');
  const reached = join(root, 'init-reached');
  mkdirSync(bin);
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'sentinel');
  writeFileSync(join(bin, 'mktemp'), `#!/usr/bin/env bash
path="$("$REAL_MKTEMP" "$@")" || exit
touch "$path/cleanup-sentinel" || exit
printf '%s\\n' "$path" >> "$MKTEMP_RECORD"
printf '%s\\n' "$path"
`, { mode: 0o755 });

  // Fail at init only after confirming both allocated directories are populated.
  const harness = `
node() { :; }
jq() { :; }
cargo() { :; }
locus() {
  if [[ "$1" == init ]]; then
    home_dir="$(sed -n '1p' "$MKTEMP_RECORD")"
    project_dir="$(sed -n '2p' "$MKTEMP_RECORD")"
    [[ "$LOCUS_HOME" == "$home_dir" && -f "$home_dir/cleanup-sentinel" && -f "$project_dir/cleanup-sentinel" ]] || return 74
    : > "$INIT_REACHED"
    return 73
  fi
}
export -f node jq cargo locus
exec bash "$1"
`;
  const result = spawnSync('bash', ['-c', harness, 'cleanup-test', script], {
    env: { PATH: `${bin}:${process.env.PATH}`, HOME: process.env.HOME, TMPDIR: scratch,
      REAL_MKTEMP: realMktemp, MKTEMP_RECORD: record, INIT_REACHED: reached },
    encoding: 'utf8',
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 73, 'the injected initialization failure must propagate');
  assert.equal(existsSync(reached), true, 'initialization must reach the injected failure');
  const paths = readFileSync(record, 'utf8').trim().split('\n');
  assert.equal(paths.length, 2);
  assert.equal(dirname(paths[0]), scratch);
  assert.equal(dirname(paths[1]), scratch);
  assert.match(paths[0].slice(scratch.length + 1), /^locus-hub-int\.[^/]+$/);
  assert.match(paths[1].slice(scratch.length + 1), /^locus-hub-int-proj\.[^/]+$/);
  assert.equal(existsSync(paths[0]), false, 'the populated temporary home must be removed');
  assert.equal(existsSync(paths[1]), false, 'the populated temporary project must be removed');
  assert.deepEqual(readdirSync(scratch), ['keep']);
  assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'sentinel');
});

test('an exit from inside the temporary project removes it', (t) => {
  const root = mkdtempSync('/tmp/lhi-cleanup-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const bin = join(root, 'bin');
  const scratch = join(root, 'scratch');
  const record = join(root, 'project');
  mkdirSync(bin);
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'sentinel');
  writeFileSync(join(bin, 'mktemp'), `#!/usr/bin/env bash
path="$("$REAL_MKTEMP" "$@")" || exit
if [[ "$path" == */locus-hub-int-proj.* ]]; then
  builtin printf '%s\\n' "$path" > "$PROJECT_RECORD"
fi
builtin printf '%s\\n' "$path"
`, { mode: 0o755 });

  // Stub prerequisites and stop at the first write after cd into the project.
  // The full integration test above still exercises the real commands.
  const harness = `
node() { :; }
jq() { :; }
locus() { :; }
printf() {
  if [[ "${'$'}{2:-}" == '{"mcpServers":'* ]]; then
    pwd -L > "$ENTERED_RECORD"
    touch project-file
    return 1
  fi
  builtin printf "$@"
}
export -f node jq locus printf
exec bash "$1"
`;
  const entered = join(root, 'entered');
  const result = spawnSync('bash', ['-c', harness, 'cleanup-test', script], {
    env: { PATH: `${bin}:${process.env.PATH}`, HOME: process.env.HOME, TMPDIR: scratch,
      REAL_MKTEMP: realMktemp, PROJECT_RECORD: record, ENTERED_RECORD: entered },
    encoding: 'utf8',
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.notEqual(result.status, 0, 'the injected write failure must stop the script');
  const project = readFileSync(record, 'utf8').trim();
  assert.ok(project.startsWith(`${scratch}/locus-hub-int-proj.`));
  assert.equal(readFileSync(entered, 'utf8').trim(), project);
  assert.equal(existsSync(project), false, 'the populated project directory must be removed');
  assert.deepEqual(readdirSync(scratch), ['keep']);
});

test('a failure inside the populated temporary project removes both directories', (t) => {
  const root = mkdtempSync('/tmp/lhi-cleanup-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const bin = join(root, 'bin');
  const scratch = join(root, 'scratch');
  const record = join(root, 'created');
  mkdirSync(bin);
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'sentinel');
  writeFileSync(join(bin, 'mktemp'), `#!/usr/bin/env bash
path="$("$REAL_MKTEMP" "$@")" || exit
printf '%s\\n' "$path" >> "$CREATED_RECORD"
builtin printf '%s\\n' "$path"
`, { mode: 0o755 });

  // Fail at status after the real project config write, not during allocation.
  const harness = `
node() { :; }
jq() { :; }
cargo() { :; }
locus() {
  if [[ "${'$'}{1:-}" == status ]]; then
    [[ -f .mcp.json && -d "$LOCUS_HOME" ]] || return 74
    mkdir nested || return 75
    touch nested/project-file "$LOCUS_HOME/home-file" || return 76
    pwd -L > "$ENTERED_RECORD"
    return 73
  fi
}
export -f node jq cargo locus
exec bash "$1"
`;
  const entered = join(root, 'entered');
  const result = spawnSync('bash', ['-c', harness, 'cleanup-test', script], {
    env: { PATH: `${bin}:${process.env.PATH}`, HOME: process.env.HOME, TMPDIR: scratch,
      REAL_MKTEMP: realMktemp, CREATED_RECORD: record, ENTERED_RECORD: entered },
    encoding: 'utf8',
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 73, 'the injected status failure must propagate');
  const paths = readFileSync(record, 'utf8').trim().split('\n');
  assert.equal(paths.length, 2, 'both temporary directories must be created');
  const [home, project] = paths;
  assert.equal(dirname(home), scratch);
  assert.equal(dirname(project), scratch);
  assert.match(basename(home), /^locus-hub-int\.[^/]+$/);
  assert.match(basename(project), /^locus-hub-int-proj\.[^/]+$/);
  assert.equal(readFileSync(entered, 'utf8').trim(), project);
  assert.equal(existsSync(home), false, 'the populated temporary home must be removed');
  assert.equal(existsSync(project), false, 'the populated project directory must be removed');
  assert.deepEqual(readdirSync(scratch), ['keep']);
  assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'sentinel');
});

test('the canary scan reports failure for a canary under a spaced path', () => {
  // Stub prerequisites; the stub locus leaks the injected ambient canary into
  // a directory whose name contains spaces, the same way a real leak would.
  const harness = `
node() { :; }
jq() { :; }
locus() {
  case " ${'$'}* " in *" mcp "*) return 1 ;; esac
  [[ -n "${'$'}{LOCUS_HOME:-}" ]] || return 0
  mkdir -p "${'$'}LOCUS_HOME/space dir"
  printf 'placeholder\\n' > "${'$'}LOCUS_HOME/plain file"
  printf '%s\\n' "${'$'}HUB_INT_AMBIENT_CANARY" > "${'$'}LOCUS_HOME/space dir/leaked file"
}
export -f node jq locus
exec bash "$1"
`;
  const result = spawnSync('bash', ['-c', harness, 'canary-scan-test', script], {
    env: { ...process.env, HUB_INT_AMBIENT_CANARY: 'hub-int-test-sentinel-canary' },
    encoding: 'utf8',
    maxBuffer: 4 * 1024 * 1024,
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 1, 'a leaked canary must fail the run');
  assert.match(result.stdout, /^ok\s+canary scan detects a canary under a spaced path$/m);
  assert.match(result.stdout, /^FAIL\s+ambient canary leaked to disk under /m);
  assert.match(result.stderr, /space dir\/leaked file/);
  assert.match(result.stdout, /^== hub-integration-test FAILED ==$/m);
});

test('a mid-run failure reaps the live multi-tenant server and both isolated directories', async (t) => {
  const root = mkdtempSync('/tmp/lhi-mt-fail-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const bin = join(root, 'bin');
  const scratch = join(root, 'scratch');
  const record = join(root, 'created');
  const urlRecord = join(root, 'urls');
  mkdirSync(bin);
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'sentinel');
  writeFileSync(join(scratch, '.locus-hub-int-scan.unrelated'), 'hidden sentinel');
  writeFileSync(join(bin, 'mktemp'), `#!/usr/bin/env bash
path="$("$REAL_MKTEMP" "$@")" || exit
[[ -d "$path" ]] || exit 1
touch "$path/cleanup-sentinel" || exit 1
printf '%s\\n' "$path" >> "$MKTEMP_RECORD"
printf '%s\\n' "$path"
`, { mode: 0o755 });
  // Proxy real curl, recording loopback URLs, but drop the first grant-scoped
  // tools/call so the script aborts while the MT server and session are live.
  writeFileSync(join(bin, 'curl'), `#!/usr/bin/env bash
for a in "$@"; do
  case "$a" in http://127.0.0.1:*) printf '%s\\n' "$a" >> "$MT_URL_RECORD" ;; esac
done
case " $* " in *locus_whoami*) exit 7 ;; esac
exec "$REAL_CURL" "$@"
`, { mode: 0o755 });

  const result = spawnSync('bash', [script], {
    env: { ...process.env, TMPDIR: scratch, PATH: `${bin}:${process.env.PATH}`,
      REAL_MKTEMP: realMktemp, REAL_CURL: realCurl,
      MKTEMP_RECORD: record, MT_URL_RECORD: urlRecord },
    encoding: 'utf8',
    maxBuffer: 4 * 1024 * 1024,
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.doesNotMatch(result.stdout, /^skip\s/m, 'the multi-tenant smoke must run');
  assert.match(result.stdout, /^ok\s+mt: server healthy on 127\.0\.0\.1:/m);
  assert.match(result.stdout, /^ok\s+mt: initialize minted grant-bound Mcp-Session-Id$/m);
  assert.doesNotMatch(result.stdout, /mt: tools\/call locus_whoami/, 'the injected failure must hit at the whoami call');
  assert.equal(result.status, 7, 'the injected transport failure must propagate');

  const paths = readFileSync(record, 'utf8').trim().split('\n');
  assert.equal(paths.length, 2);
  assert.match(paths[0], new RegExp(`^${scratch}/locus-hub-int\\.[^/]+$`));
  assert.match(paths[1], new RegExp(`^${scratch}/locus-hub-int-proj\\.[^/]+$`));
  for (const path of paths) {
    assert.equal(existsSync(path), false, `populated temporary directory must be removed: ${path}`);
  }
  assert.deepEqual(readdirSync(scratch).sort(), ['.locus-hub-int-scan.unrelated', 'keep']);

  // The trap must kill the server too: its recorded port refuses connections.
  const urls = readFileSync(urlRecord, 'utf8').trim().split('\n').filter(Boolean);
  assert.ok(urls.length > 0, 'the multi-tenant server must have been contacted');
  const { hostname, port } = new URL(urls[0]);
  let alive = true;
  for (let i = 0; i < 30 && alive; i++) {
    alive = await new Promise((resolve) => {
      const sock = net.connect({ host: hostname, port: Number(port) });
      sock.once('connect', () => { sock.destroy(); resolve(true); });
      sock.once('error', () => resolve(false));
      sock.setTimeout(500, () => { sock.destroy(); resolve(false); });
    });
    if (alive) await new Promise((r) => setTimeout(r, 100));
  }
  if (alive) spawnSync('pkill', ['-f', `locus-mcp --http ${hostname}:${port}`]);
  assert.equal(alive, false, 'the live multi-tenant server must be killed by the cleanup trap');
});

test('a failed check still removes both isolated directories on exit', (t) => {
  const root = mkdtempSync('/tmp/lhi-check-fail-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const bin = join(root, 'bin');
  const scratch = join(root, 'scratch with spaces');
  const record = join(root, 'created');
  mkdirSync(bin);
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'sentinel');
  writeFileSync(join(bin, 'mktemp'), `#!/usr/bin/env bash
path="$("$REAL_MKTEMP" "$@")" || exit
[[ -d "$path" ]] || exit 1
touch "$path/cleanup-sentinel" || exit 1
printf '%s\\n' "$path" >> "$MKTEMP_RECORD"
printf '%s\\n' "$path"
`, { mode: 0o755 });

  // Real commands, except status --oneline reports frozen so a check fails;
  // the run finishes every section then exits 1 through the fail flag.
  const harness = `
locus() {
  if [[ "$1" == status && "$2" == --oneline ]]; then
    printf 'frozen\\n'
    return 0
  fi
  command locus "$@"
}
export -f locus
exec bash "$1"
`;
  const result = spawnSync('bash', ['-c', harness, 'fail-check-test', script], {
    env: { ...process.env, TMPDIR: scratch, PATH: `${bin}:${process.env.PATH}`,
      REAL_MKTEMP: realMktemp, MKTEMP_RECORD: record },
    encoding: 'utf8',
    maxBuffer: 4 * 1024 * 1024,
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.match(result.stdout, /^FAIL\s+status --oneline expected alias:tenant, got: frozen$/m);
  assert.match(result.stdout, /^FAIL\s+expected unpinned after leave, got: frozen$/m);
  assert.match(result.stdout, /^ok\s+mt: live drop-in slice /m, 'later sections must still run to completion');
  assert.match(result.stdout, /^== hub-integration-test FAILED ==$/m);
  assert.equal(result.status, 1, 'the failed check must exit through the fail flag');

  const paths = readFileSync(record, 'utf8').trim().split('\n');
  assert.equal(paths.length, 3);
  assert.ok(paths.slice(0, 2).every((path) => dirname(path) === scratch && path.includes(' ')));
  assert.match(paths[0].slice(scratch.length + 1), /^locus-hub-int\.[^/]+$/);
  assert.match(paths[1].slice(scratch.length + 1), /^locus-hub-int-proj\.[^/]+$/);
  assert.equal(dirname(paths[2]), join(paths[1], 'tmp with spaces'),
    'the spaced-TMPDIR probe allocates beneath the project');
  for (const path of paths) {
    assert.equal(existsSync(path), false, `populated temporary directory must be removed: ${path}`);
  }
  assert.deepEqual(readdirSync(scratch), ['keep']);
  assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'sentinel');
});

test('an injected failure removes the created project directory', (t) => {
  const root = mkdtempSync('/tmp/lhi-proj-fail-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const bin = join(root, 'bin');
  const scratch = join(root, 'scratch');
  const state = join(root, 'pin-state');
  const record = join(root, 'created');
  const observed = join(root, 'project-at-cleanup');
  mkdirSync(bin);
  mkdirSync(scratch);
  mkdirSync(state);
  writeFileSync(join(scratch, 'keep'), 'sentinel');
  writeFileSync(join(bin, 'mktemp'), `#!/usr/bin/env bash
path="$("$REAL_MKTEMP" "$@")" || exit
[[ -d "$path" ]] || exit 1
printf '%s\\n' "$path" >> "$MKTEMP_RECORD"
printf '%s\\n' "$path"
`, { mode: 0o755 });
  // When the trap removes the project, the shell is still inside it and both
  // files the script wrote there (.mcp.json, hub-merged.mcp.json) must be
  // present: removal must target the populated project, not an empty husk.
  writeFileSync(join(bin, 'rm'), `#!/usr/bin/env bash
project="$(sed -n '2p' "$MKTEMP_RECORD" 2>/dev/null)"
for arg in "$@"; do
  if [[ -n "$project" && "$arg" == "$project" && "$PWD" -ef "$project" \\
      && -f "$project/.mcp.json" && -f "$project/hub-merged.mcp.json" ]]; then
    cat "$project/.mcp.json" > "$PROJECT_OBSERVED"
  fi
done
exec "$REAL_RM" "$@"
`, { mode: 0o755 });
  // Presence only: lets `command -v locus-mcp` pass so the multi-tenant
  // section runs; the injected `locus mcp mint` failure lands before launch.
  writeFileSync(join(bin, 'locus-mcp'), '#!/usr/bin/env bash\nexit 0\n', { mode: 0o755 });

  // Stub prerequisites with canned contract output; the run stays green until
  // `locus mcp mint` fails inside the populated project — an unguarded call in
  // the multi-tenant section, before the server starts (MT_SRV_PID empty).
  const mintEnv = '"env":{"LOCUS_BINDING_ID":"bid-personal","LOCUS_BINDING":"personal","LOCUS_SESSION_ID":"ses_stub","LOCUS_TENANT":"personal","LOCUS_SEAL":"hmac-sha256:stub","LOCUS_WORKER_HOME":"/tmp/stub-worker","LOCUS_EXECUTOR_CAPABILITY":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"';
  const mint = (expires) => `{"binding":"personal","binding_id":"bid-personal","session_id":"ses_stub","tenant":"personal","seal":"hmac-sha256:stub","worker_home":"/tmp/stub-worker","expires_at":"${expires}","secrets_resolved":false,${mintEnv},"LOCUS_EXPIRES_AT":"${expires}"}}`;
  const harness = `
node() {
  if [[ -n "${'$'}{HUB_INT_MCP_OUT:-}" ]]; then
    printf '%s\\n' '{"mcpServers":{"phantom":{"command":"phantom-mcp","args":[]},"locus":{"command":"locus-mcp","args":[]}}}' > "$HUB_INT_MCP_OUT"
  fi
  [[ "${'$'}{HUB_MINT_JSON:-}" != *1970-01-01* ]]
}
cargo() { :; }
locus() {
  case "${'$'}{1:-}" in
    init) ;;
    pin) rm -f "$STUB_STATE/left" ;;
    leave) : > "$STUB_STATE/left" ;;
    status)
      if [[ "${'$'}{2:-}" == --oneline ]]; then
        if [[ -f "$STUB_STATE/left" ]]; then printf 'unpinned\\n'; else printf 'personal:tenant\\n'; fi
      else printf '%s\\n' '{}'; fi ;;
    agent)
      if [[ -f "$STUB_STATE/left" ]]; then
        printf '%s\\n' '{"status":"protected","ready":false,"status_oneline":"unpinned","required_servers":["locus","phantom"],"mcp_command":"locus-mcp"}'
      else
        printf '%s\\n' '{"status":"ready","ready":true,"status_oneline":"personal:tenant","required_servers":["locus","phantom"],"mcp_command":"locus-mcp"}'
      fi ;;
    ci)
      if [[ "${'$'}{2:-}" == mint ]]; then
        if [[ " $* " == *' --ttl '* ]]; then
          printf '%s\\n' '${mint('1970-01-01T00:00:00Z')}'
        else
          printf '%s\\n' '${mint('2030-01-01T00:00:00Z')}'
        fi
      else printf '%s\\n' '{}'; fi ;;
    mcp)
      if [[ "${'$'}{2:-}" == mint && "${'$'}{3:-}" != --help ]]; then return 73; fi
      printf '%s\\n' '{}' ;;
    *) printf '%s\\n' '{}' ;;
  esac
}
export -f node cargo locus
exec bash "$1"
`;
  const result = spawnSync('bash', ['-c', harness, 'cleanup-test', script], {
    env: { ...process.env, TMPDIR: scratch, PATH: `${bin}:${process.env.PATH}`,
      REAL_MKTEMP: realMktemp, REAL_RM: realRm, MKTEMP_RECORD: record,
      PROJECT_OBSERVED: observed, STUB_STATE: state },
    encoding: 'utf8',
    maxBuffer: 4 * 1024 * 1024,
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 73, 'the injected grant mint failure must propagate');
  assert.doesNotMatch(result.stdout, /^FAIL\s/m, 'no check may fail before the injected fault');
  assert.match(result.stdout, /^ok\s+Hub mint is exact-binding and exact-session consistent$/m);
  assert.match(result.stdout, /^ok\s+Hub rejects expired broker-backed session before callback$/m);
  assert.doesNotMatch(result.stdout, /== hub-integration-test (OK|FAILED) ==/,
    'the injected failure must abort before either final banner');

  const paths = readFileSync(record, 'utf8').trim().split('\n');
  assert.equal(paths.length, 2, 'both temporary directories must be created');
  const [home, project] = paths;
  assert.equal(dirname(home), scratch);
  assert.equal(dirname(project), scratch);
  assert.match(basename(home), /^locus-hub-int\.[^/]+$/);
  assert.match(basename(project), /^locus-hub-int-proj\.[^/]+$/);
  assert.equal(existsSync(observed), true,
    'cleanup must remove the project while still populated inside it');
  assert.deepEqual(JSON.parse(readFileSync(observed, 'utf8')), {
    mcpServers: { locus: { command: 'locus-mcp', args: [] } },
  }, 'the removed project must contain its MCP registration');
  assert.equal(existsSync(project), false, 'the created project directory must be removed');
  assert.equal(existsSync(home), false, 'the created temporary home must be removed');
  assert.deepEqual(readdirSync(scratch), ['keep']);
  assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'sentinel');
});
