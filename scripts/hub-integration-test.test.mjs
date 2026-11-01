// Run: node --test scripts/hub-integration-test.test.mjs
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import net from 'node:net';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-integration-test.sh');
const realMktemp = execFileSync('which', ['mktemp'], { encoding: 'utf8' }).trim();
const realRm = execFileSync('which', ['rm'], { encoding: 'utf8' }).trim();
const realCurl = execFileSync('which', ['curl'], { encoding: 'utf8' }).trim();

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
  const initializedHomeRecord = join(root, 'initialized-home-observed');
  const projectAttempt = join(root, 'project-attempt');
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
      if [[ -f "${'$'}home_dir/bindings/personal.toml" ]]; then : > "${'$'}INITIALIZED_HOME_RECORD"; fi
      : > "${'$'}CLEANUP_RECORD"
      break
    fi
  done
fi
exec "${'$'}REAL_RM" "${'$'}@"
`, { mode: 0o755 });
  const result = spawnSync('bash', [script], {
    env: { ...process.env, TMPDIR: scratch, PATH: `${bin}:${process.env.PATH}`,
      REAL_MKTEMP: realMktemp, MKTEMP_RECORD: record, PROJECT_ATTEMPT: projectAttempt,
      FAIL_PROJECT: failProject ? '1' : '0', REAL_RM: realRm, CLEANUP_RECORD: cleanupRecord,
      INITIALIZED_HOME_RECORD: initializedHomeRecord },
    encoding: 'utf8',
    maxBuffer: 4 * 1024 * 1024,
  });
  const paths = readFileSync(record, 'utf8').trim().split('\n');
  return { result, scratch, paths, projectAttempt, cleanupRecord, initializedHomeRecord };
}

test('integration smoke uses and cleans a nondefault TMPDIR', (t) => {
  const { result, scratch, paths } = runWithTempDir(t);
  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 0, 'integration smoke must succeed');
  assert.doesNotMatch(result.stdout, /^skip\s/m, 'all integration checks must run');
  assert.match(result.stdout, /^== hub-integration-test OK ==$/m);
  assert.equal(paths.length, 2);
  assert.match(paths[0], new RegExp(`^${scratch}/locus-hub-int\\.[^/]+$`));
  assert.match(paths[1], new RegExp(`^${scratch}/locus-hub-int-proj\\.[^/]+$`));
  for (const path of paths) {
    assert.equal(existsSync(path), false, `populated temporary directory must be removed: ${path}`);
  }
  assert.deepEqual(readdirSync(scratch).sort(), ['.locus-hub-int-scan.unrelated', 'keep']);
  assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'sentinel');
  assert.equal(readFileSync(join(scratch, '.locus-hub-int-scan.unrelated'), 'utf8'), 'hidden sentinel');
});

test('normal exit removes both directories that were populated at cleanup', (t) => {
  const { result, paths, cleanupRecord, initializedHomeRecord } = runWithTempDir(t);
  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 0, 'integration smoke must finish normally');
  assert.doesNotMatch(result.stdout, /^skip\s/m, 'all integration checks must run');
  assert.match(result.stdout, /^== hub-integration-test OK ==$/m);
  assert.equal(paths.length, 2, 'both temporary directories must be allocated');
  assert.ok(result.stdout.split('\n').includes(`== hub-integration-test LOCUS_HOME=${paths[0]} ==`),
    'the allocated home must be the home used by the smoke script');
  assert.equal(existsSync(cleanupRecord), true, 'exit cleanup must receive both populated directories');
  assert.equal(existsSync(initializedHomeRecord), true, 'the home must contain the initialized binding at cleanup');
  assert.equal(existsSync(paths[0]), false, 'temporary home must be removed');
  assert.equal(existsSync(paths[1]), false, 'temporary project must be removed');
});

test('integration smoke uses and cleans TMPDIR with spaces and shell metacharacters', (t) => {
  const { result, scratch, paths } = runWithTempDir(t, false, ['a [b]', 'c d']);
  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 0, 'integration smoke must succeed');
  assert.doesNotMatch(result.stdout, /^skip\s/m, 'all integration checks must run');
  assert.equal(paths.length, 2);
  assert.ok(paths.every((path) => dirname(path) === scratch && path.includes(' ')));
  assert.match(paths[0].slice(scratch.length + 1), /^locus-hub-int\.[^/]+$/);
  assert.match(paths[1].slice(scratch.length + 1), /^locus-hub-int-proj\.[^/]+$/);
  assert.ok(result.stdout.includes(`registerLocusInMcpConfig merge write (${paths[1]}/hub-merged.mcp.json)`));
  assert.match(result.stdout, /^ok\s+mt: live drop-in slice /m);
  assert.match(result.stdout, /^ok\s+canary scan detects a canary under a spaced path$/m);
  assert.match(result.stdout, /^ok\s+ambient canary absent under /m);
  assert.match(result.stdout, /^== hub-integration-test OK ==$/m);
  for (const path of paths) {
    assert.equal(existsSync(path), false, `populated temporary directory must be removed: ${path}`);
  }
  assert.deepEqual(readdirSync(scratch).sort(), ['.locus-hub-int-scan.unrelated', 'keep']);
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
  assert.equal(paths.length, 2);
  assert.ok(paths.every((path) => dirname(path) === scratch && path.includes(' ')));
  assert.match(paths[0].slice(scratch.length + 1), /^locus-hub-int\.[^/]+$/);
  assert.match(paths[1].slice(scratch.length + 1), /^locus-hub-int-proj\.[^/]+$/);
  for (const path of paths) {
    assert.equal(existsSync(path), false, `populated temporary directory must be removed: ${path}`);
  }
  assert.deepEqual(readdirSync(scratch), ['keep']);
  assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'sentinel');
});
