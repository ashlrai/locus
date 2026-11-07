// Run: node --test scripts/e2e.test.mjs
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { existsSync, lstatSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const script = join(root, 'scripts', 'e2e.sh');
const locus = join(root, 'target', 'release', 'locus');

test('temporary home and workspace are removed after a successful e2e exit', { timeout: 1_800_000 }, (t) => {
  const fixture = mkdtempSync(join(tmpdir(), 'locus-e2e-success-test-'));
  t.after(() => rmSync(fixture, { recursive: true, force: true }));
  const scratch = join(fixture, 'custom tmp $literal; [test]');
  const inherited = join(fixture, 'inherited home');
  const bin = join(fixture, 'bin');
  const homeCreated = join(fixture, 'home-created');
  const homeUsed = join(fixture, 'home-used');
  const cleanupObserved = join(fixture, 'cleanup-observed');
  const workspaceCreated = join(fixture, 'workspace-created');
  const workspaceUsed = join(fixture, 'workspace-used');
  const wrapper = join(bin, 'locus');
  mkdirSync(scratch);
  mkdirSync(inherited);
  mkdirSync(bin);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  writeFileSync(join(inherited, 'keep'), 'inherited');
  const realMktemp = execFileSync('which', ['mktemp'], { encoding: 'utf8' }).trim();
  const realRm = execFileSync('which', ['rm'], { encoding: 'utf8' }).trim();
  writeFileSync(join(bin, 'rm'), `#!/bin/bash
if [[ -f "$HOME_CREATED" ]]; then
  IFS= read -r home < "$HOME_CREATED" || [[ -n "$home" ]]
  for arg in "$@"; do
    if [[ "$arg" == "$home" ]]; then
      [[ -f "$home/cleanup-sentinel" ]] || exit 1
      printf '%s' "$home" > "$CLEANUP_OBSERVED"
    fi
  done
fi
exec "$REAL_RM" "$@"
`, { mode: 0o755 });
  writeFileSync(join(bin, 'mktemp'), `#!/bin/bash
case "\${2:-}" in
  "$SUITE_TMPDIR/locus-e2e.XXXXXX") record="$HOME_CREATED" ;;
  "$SUITE_TMPDIR/locus-ws.XXXXXX") record="$WORKSPACE_CREATED" ;;
  *) exec "$REAL_MKTEMP" "$@" ;;
esac
[[ "$#" -eq 2 && "$1" == -d ]] || exit 1
created="$("$REAL_MKTEMP" "$@")" || exit $?
[[ -d "$created" ]] || exit 1
printf '%s' "$created" > "$record"
printf '%s\\n' "$created"
`, { mode: 0o755 });
  // Rehearsal subshells re-export TMPDIR to a nested root; observe the home
  // only at the suite's own workspace call, where LOCUS_HOME still names the
  // suite home beneath the outer root.
  writeFileSync(wrapper, `#!/bin/bash
if [[ "\${1:-}" == workspace && "$LOCUS_HOME" == "$SUITE_TMPDIR"/locus-e2e.* ]]; then
  [[ -d "$LOCUS_HOME" ]] || exit 1
  printf '%s' "$LOCUS_HOME" > "$HOME_USED"
  [[ -d "$PWD" && "$PWD" == "$(cat "$WORKSPACE_CREATED")" ]] || exit 1
  printf '%s' "$PWD" > "$WORKSPACE_USED"
  touch "$PWD/cleanup-sentinel" || exit 1
  touch "$LOCUS_HOME/cleanup-sentinel" || exit 1
fi
exec '${locus.replaceAll("'", "'\\''")}' "$@"
`, { mode: 0o755 });

  const result = spawnSync('bash', [script], {
    cwd: root,
    env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, TMPDIR: scratch,
      SUITE_TMPDIR: scratch,
      LOCUS_HOME: inherited, LOCUS_BIN: wrapper, HOME_CREATED: homeCreated,
      HOME_USED: homeUsed, CLEANUP_OBSERVED: cleanupObserved, WORKSPACE_CREATED: workspaceCreated,
      WORKSPACE_USED: workspaceUsed, REAL_MKTEMP: realMktemp, REAL_RM: realRm },
    encoding: 'utf8',
    timeout: 1_800_000,
    maxBuffer: 10 * 1024 * 1024,
  });
  assert.equal(result.signal, null, 'e2e process terminated unexpectedly');
  assert.equal(result.status, 0, 'e2e should exit successfully');
  assert.match(result.stdout, /e2e PASS  \(\d+ checks, \d+ skipped\)/);
  assert.ok(existsSync(homeCreated), 'mktemp did not create the temporary home');
  assert.ok(existsSync(homeUsed), 'the workspace command did not observe the temporary home');
  const home = readFileSync(homeCreated, 'utf8');
  assert.equal(readFileSync(homeUsed, 'utf8'), home, 'CLI used a different home');
  assert.equal(dirname(home), scratch);
  assert.match(home, /\/locus-e2e\.[^/]+$/);
  assert.ok(existsSync(cleanupObserved), 'exit cleanup did not receive the populated home');
  assert.equal(readFileSync(cleanupObserved, 'utf8'), home);
  assert.throws(() => lstatSync(home), { code: 'ENOENT' }, 'temporary home survived exit cleanup');
  assert.ok(existsSync(workspaceCreated), 'mktemp did not create the workspace');
  assert.ok(existsSync(workspaceUsed), 'the workspace command did not enter the workspace');
  const workspace = readFileSync(workspaceCreated, 'utf8');
  assert.equal(readFileSync(workspaceUsed, 'utf8'), workspace, 'CLI used a different workspace');
  assert.equal(dirname(workspace), scratch);
  assert.match(workspace, /\/locus-ws\.[^/]+$/);
  assert.equal(existsSync(workspace), false, 'workspace survived exit cleanup');
  assert.deepEqual(readdirSync(scratch), ['keep'], 'cleanup removed or left unexpected files');
  assert.equal(readFileSync(join(inherited, 'keep'), 'utf8'), 'inherited',
    'cleanup must preserve the inherited home');
});

test('home and workspace honor a spaced TMPDIR and are cleaned up on failure', { timeout: 1_800_000 }, (t) => {
  const fixture = mkdtempSync(join(tmpdir(), 'locus-e2e-workspace-test-'));
  t.after(() => rmSync(fixture, { recursive: true, force: true }));
  const scratch = join(fixture, 'custom tmp with spaces');
  const bin = join(fixture, 'bin');
  const created = join(fixture, 'workspace-created');
  const entered = join(fixture, 'workspace-entered');
  const homeCreated = join(fixture, 'home-created');
  const homeUsed = join(fixture, 'home-used');
  mkdirSync(scratch);
  mkdirSync(bin);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  const realMktemp = execFileSync('which', ['mktemp'], { encoding: 'utf8' }).trim();
  // Each fixture call must still arrive as `-d` plus the current root's
  // template, but rehearsal subshells re-export TMPDIR to a nested root; only
  // creations directly beneath the outer root are the suite's own fixtures.
  writeFileSync(join(bin, 'mktemp'), `#!/bin/bash
case "$*" in
  *locus-e2e.XXXXXX*)
    [[ $# -eq 2 && "$1" == -d && "$2" == "$TMPDIR/locus-e2e.XXXXXX" ]] || exit 1
    home="$("$REAL_MKTEMP" "$@")" || exit $?
    if [[ "$2" == "$SUITE_TMPDIR/locus-e2e.XXXXXX" ]]; then
      printf '%s' "$home" > "$HOME_CREATED"
    fi
    printf '%s\\n' "$home"
    ;;
  *locus-ws.XXXXXX*)
    [[ $# -eq 2 && "$1" == -d && "$2" == "$TMPDIR/locus-ws.XXXXXX" ]] || exit 1
    workspace="$("$REAL_MKTEMP" "$@")" || exit $?
    if [[ "$2" == "$SUITE_TMPDIR/locus-ws.XXXXXX" ]]; then
      printf '%s' "$workspace" > "$WORKSPACE_CREATED"
    fi
    printf '%s\\n' "$workspace"
    ;;
  *) exec "$REAL_MKTEMP" "$@" ;;
esac
`, { mode: 0o755 });
  const wrapper = join(bin, 'locus');
  // Rehearsal subshells re-export TMPDIR to a nested root and call `workspace`
  // there; inject the failure only at the suite's own workspace call, which
  // runs inside a workspace directly beneath the outer root.
  writeFileSync(wrapper, `#!/bin/bash
if [[ "\${1:-}" == workspace && "\${PWD%/*}" == "$SUITE_TMPDIR" ]]; then
  [[ -d "$PWD" ]] || exit 1
  [[ -d "$LOCUS_HOME" ]] || exit 1
  printf '%s' "$PWD" > "$WORKSPACE_ENTERED"
  printf '%s' "$LOCUS_HOME" > "$HOME_USED"
  touch "$PWD/cleanup-sentinel"
  exit 1
fi
exec '${locus.replaceAll("'", "'\\''")}' "$@"
`, { mode: 0o755 });

  const result = spawnSync('bash', [script], {
    cwd: root,
    env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, TMPDIR: scratch,
      SUITE_TMPDIR: scratch,
      LOCUS_BIN: wrapper, REAL_MKTEMP: realMktemp, WORKSPACE_CREATED: created,
      WORKSPACE_ENTERED: entered, HOME_CREATED: homeCreated, HOME_USED: homeUsed },
    encoding: 'utf8',
    timeout: 1_800_000,
    maxBuffer: 10 * 1024 * 1024,
  });
  assert.equal(result.signal, null, 'e2e process terminated unexpectedly');
  assert.equal(result.status, 1, 'the injected workspace failure should stop e2e');
  assert.match(result.stdout, /==> 4\. workspace allowlist/);
  assert.ok(existsSync(created), 'workspace mktemp did not receive the expected arguments');
  assert.ok(existsSync(entered), 'the workspace command was not reached');
  assert.ok(existsSync(homeCreated), 'home mktemp did not receive the expected arguments');
  assert.ok(existsSync(homeUsed), 'the workspace command did not observe a home');
  const home = readFileSync(homeCreated, 'utf8');
  assert.equal(dirname(home), scratch);
  assert.match(home, /\/locus-e2e\.[^/]+$/);
  assert.equal(readFileSync(homeUsed, 'utf8'), home, 'command used a different home');
  assert.equal(existsSync(home), false, 'home survived exit cleanup');
  const workspace = readFileSync(created, 'utf8');
  assert.equal(dirname(workspace), scratch);
  assert.match(workspace, /\/locus-ws\.[^/]+$/);
  assert.equal(readFileSync(entered, 'utf8'), workspace, 'command entered a different directory');
  assert.equal(existsSync(workspace), false, 'workspace survived exit cleanup');
  assert.deepEqual(readdirSync(scratch), ['keep'], 'cleanup removed or left unexpected files');
});

test('a later workspace command failure removes the populated workspace', { timeout: 1_800_000 }, (t) => {
  const fixture = mkdtempSync(join(tmpdir(), 'locus-e2e-workspace-late-failure-'));
  t.after(() => rmSync(fixture, { recursive: true, force: true }));
  const scratch = join(fixture, 'tmp $literal; [test]');
  const bin = join(fixture, 'bin');
  const recorded = join(fixture, 'workspace-at-failure');
  const inherited = join(fixture, 'inherited workspace');
  mkdirSync(scratch);
  mkdirSync(bin);
  mkdirSync(inherited);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  writeFileSync(join(inherited, 'keep'), 'inherited');
  const wrapper = join(bin, 'locus');
  writeFileSync(wrapper, `#!/bin/bash
if [[ "\${1:-}" == pin && "\${2:-}" == personal && "\${3:-}" == --force && "$PWD" == "$TMPDIR"/locus-ws.* ]]; then
  [[ -f "$PWD/.locus.toml" ]] || exit 74
  touch "$PWD/cleanup-sentinel" || exit 74
  printf '%s' "$PWD" > "$WORKSPACE_AT_FAILURE"
  exit 73
fi
exec '${locus.replaceAll("'", "'\\''")}' "$@"
`, { mode: 0o755 });

  const result = spawnSync('bash', [script], {
    cwd: root,
    env: { ...process.env, TMPDIR: scratch, WS_DIR: inherited,
      LOCUS_BIN: wrapper, WORKSPACE_AT_FAILURE: recorded },
    encoding: 'utf8',
    timeout: 1_800_000,
    maxBuffer: 10 * 1024 * 1024,
  });
  assert.equal(result.signal, null, 'e2e process terminated unexpectedly');
  assert.equal(result.status, 73, 'cleanup must preserve the injected command failure');
  assert.match(result.stdout, /ok  personal blocked without --force/);
  assert.ok(existsSync(recorded), 'failure was not reached in the populated workspace');
  const workspace = readFileSync(recorded, 'utf8');
  assert.equal(dirname(workspace), scratch);
  assert.match(workspace, /\/locus-ws\.[^/]+$/);
  assert.equal(existsSync(workspace), false, 'populated workspace survived exit cleanup');
  assert.deepEqual(readdirSync(scratch), ['keep'], 'cleanup removed or left unexpected files');
  assert.equal(readFileSync(join(inherited, 'keep'), 'utf8'), 'inherited');
});

test('binding add regression detects changes to an existing matching file', { timeout: 1_800_000 }, (t) => {
  const fixture = mkdtempSync(join(tmpdir(), 'locus-e2e-binding-test-'));
  t.after(() => rmSync(fixture, { recursive: true, force: true }));
  const wrapper = join(fixture, 'locus');
  const marker = join(fixture, 'first-add');
  writeFileSync(wrapper, `#!/bin/bash
if [[ "\${1:-}" == --json && "\${2:-}" == binding && "\${3:-}" == add && "\${4:-}" == e2e-add ]]; then
  '${locus.replaceAll("'", "'\\''")}' "$@" || exit $?
  if [[ -e '${marker.replaceAll("'", "'\\''")}' ]]; then
    printf '\\n# injected rewrite\\n' >> "$LOCUS_HOME/bindings/e2e-add.toml"
  else
    : > '${marker.replaceAll("'", "'\\''")}'
  fi
  exit 0
fi
exec '${locus.replaceAll("'", "'\\''")}' "$@"
`, { mode: 0o755 });

  const result = spawnSync('bash', [script], {
    cwd: root,
    env: { ...process.env, LOCUS_BIN: wrapper },
    encoding: 'utf8',
    timeout: 1_800_000,
    maxBuffer: 10 * 1024 * 1024,
  });
  assert.equal(result.signal, null, 'e2e process terminated unexpectedly');
  assert.equal(result.status, 1, 'e2e should reject a changed binding file');
  assert.match(result.stderr, /FAIL identical binding add changed file contents/);
  assert.doesNotMatch(result.stdout, /ok  binding add repeats without a write and rejects conflicts/);
});

for (const failure of ['none', 'init', 'import']) {
  test(`graph fallback uses TMPDIR and cleans up ${failure === 'none' ? 'after import' : `after ${failure} failure`}`, { timeout: 1_800_000 }, (t) => {
    const fixture = mkdtempSync(join(tmpdir(), 'locus-e2e-test-'));
    t.after(() => rmSync(fixture, { recursive: true, force: true }));
    const scratch = join(fixture, 'custom tmp');
    const bin = join(fixture, 'bin');
    const observed = join(fixture, 'graph-home');
    const imported = join(fixture, 'import-observed');
    const initialized = join(fixture, 'init-observed');
    const attempts = join(fixture, 'same-home-attempts');
    const cleanedEarly = join(fixture, 'cleaned-before-next-section');
    mkdirSync(scratch);
    mkdirSync(bin);
    writeFileSync(join(scratch, 'keep'), 'unrelated');

    // Force both same-home attempts to fail, then observe the real fresh-home import.
    // Inject only at the suite's own graph home: the section 2i rehearsal
    // re-exports TMPDIR to a nested root, so only a home directly beneath the
    // OUTER root is the fallback fixture this test means to steer.
    const wrapper = join(bin, 'locus');
    writeFileSync(wrapper, `#!/bin/bash
if [[ "\${1:-}" == init && "$LOCUS_HOME" == "$E2E_OUTER_TMPDIR"/locus-graph-imp.* ]]; then
  [[ -d "$LOCUS_HOME" && "$LOCUS_GRAPH_PASSPHRASE" == e2e-test ]] || exit 1
  printf '%s' "$LOCUS_HOME" > "$GRAPH_INIT_RECORD"
  [[ "$GRAPH_FAILURE" != init ]] || exit 1
fi
if [[ "\${1:-}" == graph && "\${2:-}" == import ]]; then
  if [[ "$LOCUS_HOME" != "$TMPDIR"/locus-graph-imp.* ]]; then
    if [[ "\${4:-}" == --force ]]; then
      printf 'force\\n' >> "$GRAPH_ATTEMPTS_RECORD"
    else
      printf 'normal\\n' >> "$GRAPH_ATTEMPTS_RECORD"
    fi
    exit 1
  fi
  [[ -d "$LOCUS_HOME" && "$LOCUS_GRAPH_PASSPHRASE" == e2e-test ]] || exit 1
  [[ "$(cat "$GRAPH_INIT_RECORD")" == "$LOCUS_HOME" ]] || exit 1
  printf '%s' "$LOCUS_HOME" > "$GRAPH_HOME_RECORD"
  touch "$LOCUS_HOME/import-sentinel" || exit 1
  : > "$GRAPH_IMPORT_RECORD"
  [[ "$GRAPH_FAILURE" != import ]] || exit 1
fi
if [[ "\${1:-}" == ci && "\${2:-}" == --help && -f "$GRAPH_IMPORT_RECORD" ]]; then
  graph_home="$(cat "$GRAPH_HOME_RECORD")"
  [[ ! -e "$graph_home" && ! -L "$graph_home" ]] || exit 1
  : > "$GRAPH_CLEANED_RECORD"
fi
exec '${locus.replaceAll("'", "'\\''")}' "$@"
`, { mode: 0o755 });

    const result = spawnSync('bash', [script], {
      cwd: root,
      env: {
        ...process.env,
        TMPDIR: scratch,
        E2E_OUTER_TMPDIR: scratch,
        LOCUS_BIN: wrapper,
        GRAPH_HOME_RECORD: observed,
        GRAPH_IMPORT_RECORD: imported,
        GRAPH_INIT_RECORD: initialized,
        GRAPH_ATTEMPTS_RECORD: attempts,
        GRAPH_CLEANED_RECORD: cleanedEarly,
        GRAPH_FAILURE: failure,
        LOCUS_GRAPH_PASSPHRASE: 'e2e-test',
      },
      encoding: 'utf8',
      timeout: 1_800_000,
      maxBuffer: 10 * 1024 * 1024,
    });

    assert.equal(result.signal, null, 'e2e process terminated unexpectedly');
    const stages = [...result.stdout.matchAll(/^==> (\d+[a-z]?)\./gm)];
    assert.equal(result.status, failure === 'none' ? 0 : 1,
      `unexpected e2e exit status at stage ${stages.at(-1)?.[1] ?? 'before startup'}`);
    assert.ok(existsSync(initialized), 'fresh graph initialization was not reached');
    assert.equal(readFileSync(attempts, 'utf8'), 'normal\nforce\n',
      'both same-home imports must fail before the fresh-home fallback');
    assert.equal(existsSync(imported), failure !== 'init', 'unexpected fresh graph import');
    const graphHome = readFileSync(initialized, 'utf8');
    if (failure !== 'init') assert.equal(readFileSync(observed, 'utf8'), graphHome);
    assert.ok(dirname(graphHome) === scratch, 'graph home was not created directly under TMPDIR');
    assert.ok(/\/locus-graph-imp\.[^/]+$/.test(graphHome), 'unexpected graph home name');
    assert.equal(existsSync(graphHome), false, 'graph home was not cleaned up');
    assert.deepEqual(readdirSync(scratch), ['keep'], 'cleanup removed or left unexpected files');
    if (failure !== 'none') {
      assert.ok(/graph import failed/.test(result.stderr), 'expected graph import failure');
      assert.doesNotMatch(result.stdout, /==> 17\. /, 'failure must stop before the next section');
    } else {
      assert.ok(existsSync(cleanedEarly), 'graph home survived until the next section');
      assert.match(result.stdout, /ok  sample exec succeeds without unset-variable warnings/);
      assert.match(result.stdout, /ok  sample walkthrough exec succeeds without unset-variable warnings/);
      assert.match(result.stdout, /ok  sample exec reports missing credentials/);
      assert.ok(/ok  graph export\/import into fresh LOCUS_HOME/.test(result.stdout), 'fresh import did not pass');
      assert.ok(/e2e PASS/.test(result.stdout), 'e2e did not pass');
      const summary = result.stdout.match(/e2e PASS  \((\d+) checks, (\d+) skipped\)/);
      assert.ok(summary, 'e2e summary missing');
      t.diagnostic(`e2e: ${summary[1]} checks passed, ${summary[2]} skipped`);
    }
  });
}

test('graph fallback success removes the populated import home through rm', { timeout: 1_800_000 }, (t) => {
  const fixture = mkdtempSync(join(tmpdir(), 'locus-e2e-graph-rm-test-'));
  t.after(() => rmSync(fixture, { recursive: true, force: true }));
  const scratch = join(fixture, 'custom tmp $literal; [test]');
  const bin = join(fixture, 'bin');
  const initialized = join(fixture, 'init-observed');
  const attempts = join(fixture, 'same-home-attempts');
  const rmObserved = join(fixture, 'rm-observed');
  mkdirSync(scratch);
  mkdirSync(bin);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  const realRm = execFileSync('which', ['rm'], { encoding: 'utf8' }).trim();
  // The section cleanup is `rm -rf "$GRAPH_HOME"` — a two-argument call. The
  // EXIT trap passes the graph home among several arguments, so only the
  // two-argument form proves the success-path removal carried a populated home.
  writeFileSync(join(bin, 'rm'), `#!/bin/bash
if [[ "$#" -eq 2 && "$1" == -rf && "$2" == "$TMPDIR"/locus-graph-imp.* ]]; then
  if [[ -e "$2" || -L "$2" ]]; then
    [[ -f "$2/import-sentinel" ]] || exit 1
    printf '%s' "$2" > "$GRAPH_RM_OBSERVED"
  fi
fi
exec "$REAL_RM" "$@"
`, { mode: 0o755 });
  // Inject only at the suite's own graph home (directly beneath the OUTER
  // root): the section 2i rehearsal re-exports TMPDIR to a nested root before
  // the passphrase is exported, so a nested home is not this test's fixture.
  const wrapper = join(bin, 'locus');
  writeFileSync(wrapper, `#!/bin/bash
if [[ "\${1:-}" == init && "$LOCUS_HOME" == "$E2E_OUTER_TMPDIR"/locus-graph-imp.* ]]; then
  [[ -d "$LOCUS_HOME" && "$LOCUS_GRAPH_PASSPHRASE" == e2e-test ]] || exit 1
  printf '%s' "$LOCUS_HOME" > "$GRAPH_INIT_RECORD"
fi
if [[ "\${1:-}" == graph && "\${2:-}" == import ]]; then
  if [[ "$LOCUS_HOME" != "$E2E_OUTER_TMPDIR"/locus-graph-imp.* ]]; then
    if [[ "\${4:-}" == --force ]]; then
      printf 'force\\n' >> "$GRAPH_ATTEMPTS_RECORD"
    else
      printf 'normal\\n' >> "$GRAPH_ATTEMPTS_RECORD"
    fi
    exit 1
  fi
  [[ -d "$LOCUS_HOME" && "$LOCUS_GRAPH_PASSPHRASE" == e2e-test ]] || exit 1
  [[ "$(cat "$GRAPH_INIT_RECORD")" == "$LOCUS_HOME" ]] || exit 1
  touch "$LOCUS_HOME/import-sentinel" || exit 1
fi
exec '${locus.replaceAll("'", "'\\''")}' "$@"
`, { mode: 0o755 });

  const result = spawnSync('bash', [script], {
    cwd: root,
    env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, TMPDIR: scratch,
      E2E_OUTER_TMPDIR: scratch, LOCUS_BIN: wrapper, REAL_RM: realRm,
      GRAPH_INIT_RECORD: initialized, GRAPH_ATTEMPTS_RECORD: attempts,
      GRAPH_RM_OBSERVED: rmObserved },
    encoding: 'utf8',
    timeout: 1_800_000,
    maxBuffer: 10 * 1024 * 1024,
  });

  assert.equal(result.signal, null, 'e2e process terminated unexpectedly');
  assert.equal(result.status, 0, 'e2e should exit successfully');
  assert.match(result.stdout, /e2e PASS  \(\d+ checks, \d+ skipped\)/);
  assert.equal(readFileSync(attempts, 'utf8'), 'normal\nforce\n',
    'both same-home imports must fail before the fresh-home fallback');
  assert.ok(existsSync(initialized), 'fresh graph initialization was not reached');
  const graphHome = readFileSync(initialized, 'utf8');
  assert.equal(dirname(graphHome), scratch);
  assert.match(graphHome, /\/locus-graph-imp\.[^/]+$/);
  assert.match(result.stdout, /ok  graph export\/import into fresh LOCUS_HOME/,
    'fresh import did not pass');
  assert.match(result.stdout, /ok  graph import temporary home removed/,
    'the suite did not confirm the import home removal');
  assert.ok(existsSync(rmObserved),
    'rm was not observed removing the populated import home');
  assert.equal(readFileSync(rmObserved, 'utf8'), graphHome,
    'rm removed a different directory than the initialized import home');
  assert.equal(existsSync(graphHome), false, 'graph import home survived the run');
  assert.deepEqual(readdirSync(scratch), ['keep'], 'cleanup removed or left unexpected files');
});

test('sample exec regression rejects an unset-variable warning', { timeout: 1_800_000 }, (t) => {
  const fixture = mkdtempSync(join(tmpdir(), 'locus-e2e-warning-test-'));
  t.after(() => rmSync(fixture, { recursive: true, force: true }));
  const wrapper = join(fixture, 'locus');
  writeFileSync(wrapper, `#!/bin/bash
if [[ "$*" == 'exec --expect personal -- echo ok' ]]; then
  printf '%s\\n' 'warning: synthetic unset-variable warning' >&2
fi
exec '${locus.replaceAll("'", "'\\''")}' "$@"
`, { mode: 0o755 });

  const result = spawnSync('bash', [script], {
    cwd: root,
    env: { ...process.env, LOCUS_BIN: wrapper },
    encoding: 'utf8',
    timeout: 1_800_000,
    maxBuffer: 10 * 1024 * 1024,
  });
  assert.equal(result.signal, null, 'e2e process terminated unexpectedly');
  assert.equal(result.status, 1, 'e2e should reject the injected warning');
  assert.match(result.stderr, /FAIL sample exec emitted an unset-variable warning/);
  assert.doesNotMatch(result.stdout, /ok  sample exec succeeds without unset-variable warnings/);
});

test('security fixtures are isolated and cleaned after a probe failure', { timeout: 1_800_000 }, (t) => {
  const fixture = mkdtempSync(join(tmpdir(), 'locus-e2e-security-test-'));
  t.after(() => rmSync(fixture, { recursive: true, force: true }));
  const scratch = join(fixture, 'custom tmp');
  const observed = join(fixture, 'security-home');
  const workspaceRecord = join(fixture, 'security-workspace');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  const wrapper = join(fixture, 'locus');
  writeFileSync(wrapper, `#!/bin/bash
if [[ "\${1:-}" == init && "$LOCUS_HOME" == "$TMPDIR"/locus-security-e2e.* ]]; then
  [[ -d "$LOCUS_HOME" ]] || exit 1
  printf '%s' "$LOCUS_HOME" > "$SECURITY_HOME_RECORD"
  workspaces=("$TMPDIR"/locus-security-ws.*)
  [[ \${#workspaces[@]} -eq 1 && -d "\${workspaces[0]}" ]] || exit 1
  printf '%s' "\${workspaces[0]}" > "$SECURITY_WS_RECORD"
  touch "$LOCUS_HOME/cleanup-sentinel" "\${workspaces[0]}/cleanup-sentinel"
  exit 73
fi
exec '${locus.replaceAll("'", "'\\''")}' "$@"
`, { mode: 0o755 });

  const result = spawnSync('bash', [script], {
    cwd: root,
    env: { ...process.env, TMPDIR: scratch, LOCUS_BIN: wrapper,
      SECURITY_HOME_RECORD: observed, SECURITY_WS_RECORD: workspaceRecord },
    encoding: 'utf8',
    timeout: 1_800_000,
    maxBuffer: 10 * 1024 * 1024,
  });
  assert.equal(result.signal, null, 'e2e process terminated unexpectedly');
  assert.equal(result.status, 73, 'the injected security initialization failure should stop e2e');
  assert.match(result.stdout, /==> 24\. adversarial release credential\/workspace probes/);
  const home = readFileSync(observed, 'utf8');
  const workspace = readFileSync(workspaceRecord, 'utf8');
  assert.equal(dirname(home), scratch);
  assert.equal(dirname(workspace), scratch);
  assert.match(home, /\/locus-security-e2e\.[^/]+$/);
  assert.match(workspace, /\/locus-security-ws\.[^/]+$/);
  assert.notEqual(home, workspace);
  assert.equal(existsSync(home), false, 'security home survived exit cleanup');
  assert.equal(existsSync(workspace), false, 'security workspace survived exit cleanup');
  assert.deepEqual(readdirSync(scratch), ['keep'], 'cleanup removed or left unexpected files');
});

test('security home is cleaned when workspace creation fails', { timeout: 1_800_000 }, (t) => {
  const fixture = mkdtempSync(join(tmpdir(), 'locus-e2e-security-setup-test-'));
  t.after(() => rmSync(fixture, { recursive: true, force: true }));
  const scratch = join(fixture, 'custom tmp $literal; [test]');
  const inheritedWorkspace = join(fixture, 'inherited security workspace');
  const bin = join(fixture, 'bin');
  const homeRecord = join(fixture, 'security-home');
  const failureRecord = join(fixture, 'workspace-failure');
  mkdirSync(scratch);
  mkdirSync(inheritedWorkspace);
  mkdirSync(bin);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  writeFileSync(join(inheritedWorkspace, 'keep'), 'inherited');
  const realMktemp = execFileSync('which', ['mktemp'], { encoding: 'utf8' }).trim();
  writeFileSync(join(bin, 'mktemp'), `#!/bin/bash
if [[ "$#" -eq 2 && "$1" == -d && "$TMPDIR" == "$PROBE_SCRATCH" ]]; then
  case "$2" in
    "$TMPDIR/locus-security-e2e.XXXXXX")
      home="$("$REAL_MKTEMP" "$@")" || exit $?
      [[ -d "$home" ]] || exit 1
      printf '%s' "$home" > "$SECURITY_HOME_RECORD"
      touch "$home/cleanup-sentinel" || exit 1
      printf '%s\\n' "$home"
      exit 0
      ;;
    "$TMPDIR/locus-security-ws.XXXXXX")
      [[ -f "$SECURITY_HOME_RECORD" && -f "$(cat "$SECURITY_HOME_RECORD")/cleanup-sentinel" ]] || exit 1
      : > "$SECURITY_FAILURE_RECORD"
      exit 73
      ;;
  esac
fi
exec "$REAL_MKTEMP" "$@"
`, { mode: 0o755 });

  const result = spawnSync('bash', [script], {
    cwd: root,
    env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, TMPDIR: scratch,
      SECURITY_WS: inheritedWorkspace, REAL_MKTEMP: realMktemp, PROBE_SCRATCH: scratch,
      SECURITY_HOME_RECORD: homeRecord, SECURITY_FAILURE_RECORD: failureRecord },
    encoding: 'utf8',
    timeout: 1_800_000,
    maxBuffer: 10 * 1024 * 1024,
  });
  assert.equal(result.signal, null, 'e2e process terminated unexpectedly');
  assert.equal(result.status, 73, 'the injected workspace creation failure should stop e2e');
  assert.match(result.stdout, /==> 24\. adversarial release credential\/workspace probes/);
  assert.ok(existsSync(failureRecord), 'workspace creation failure was not reached');
  const home = readFileSync(homeRecord, 'utf8');
  assert.equal(dirname(home), scratch);
  assert.match(home, /\/locus-security-e2e\.[^/]+$/);
  assert.equal(existsSync(home), false, 'security home and its sentinel survived exit cleanup');
  assert.deepEqual(readdirSync(scratch), ['keep'], 'cleanup removed or left unexpected files');
  assert.equal(readFileSync(join(inheritedWorkspace, 'keep'), 'utf8'), 'inherited');
});

test('security workspace with its broken policy link is removed after a late probe failure', { timeout: 1_800_000 }, (t) => {
  const fixture = mkdtempSync(join(tmpdir(), 'locus-e2e-security-ws-late-test-'));
  t.after(() => rmSync(fixture, { recursive: true, force: true }));
  const scratch = join(fixture, 'custom tmp');
  const workspaceRecord = join(fixture, 'security-workspace');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  // Fail the doctor probe, which runs inside the workspace after the dangling
  // .locus.toml symlink and other fixtures have been written.
  const wrapper = join(fixture, 'locus');
  writeFileSync(wrapper, `#!/bin/bash
if [[ "\${1:-}" == --json && "\${2:-}" == doctor && "$PWD" == "$TMPDIR"/locus-security-ws.* ]]; then
  [[ -L "$PWD/.locus.toml" ]] || exit 1
  printf '%s' "$PWD" > "$SECURITY_WS_RECORD"
  touch "$PWD/cleanup-sentinel"
  exit 74
fi
exec '${locus.replaceAll("'", "'\\''")}' "$@"
`, { mode: 0o755 });

  const result = spawnSync('bash', [script], {
    cwd: root,
    env: { ...process.env, TMPDIR: scratch, LOCUS_BIN: wrapper, SECURITY_WS_RECORD: workspaceRecord },
    encoding: 'utf8',
    timeout: 1_800_000,
    maxBuffer: 10 * 1024 * 1024,
  });
  assert.equal(result.signal, null, 'e2e process terminated unexpectedly');
  // 74 is not doctor's UNSAFE status (2), so the script's own check fails with 1.
  assert.equal(result.status, 1, 'the injected doctor failure should stop e2e');
  assert.match(result.stderr, /FAIL doctor did not report UNSAFE for broken workspace link/);
  const workspace = readFileSync(workspaceRecord, 'utf8');
  assert.equal(dirname(workspace), scratch);
  assert.match(workspace, /\/locus-security-ws\.[^/]+$/);
  assert.equal(existsSync(workspace), false, 'security workspace survived exit cleanup');
  assert.deepEqual(readdirSync(scratch), ['keep'], 'cleanup removed or left unexpected files');
});

test('security workspace creation lands at the intended path beneath a spaced TMPDIR', { timeout: 1_800_000 }, (t) => {
  const fixture = mkdtempSync(join(tmpdir(), 'locus-e2e-security-ws-path-test-'));
  t.after(() => rmSync(fixture, { recursive: true, force: true }));
  const scratch = join(fixture, 'security ws tmp $literal; [test]');
  const bin = join(fixture, 'bin');
  const created = join(fixture, 'security-ws-created');
  const templates = join(fixture, 'security-ws-templates');
  const entered = join(fixture, 'security-ws-entered');
  const used = join(fixture, 'security-ws-used');
  mkdirSync(scratch);
  mkdirSync(bin);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  const realMktemp = execFileSync('which', ['mktemp'], { encoding: 'utf8' }).trim();
  // Every locus-security-ws template is logged; the probe creation beneath the
  // suite TMPDIR must arrive as exactly `-d` plus one quoted template, and the
  // directory mktemp returns must sit directly under the spaced root.
  writeFileSync(join(bin, 'mktemp'), `#!/bin/bash
if [[ $# -eq 2 && "$1" == -d && "$2" == */locus-security-ws.XXXXXX ]]; then
  printf '%s\\n' "$2" >> "$SECURITY_WS_TEMPLATES"
fi
if [[ $# -eq 2 && "$1" == -d && "$2" == "$PROBE_SCRATCH/locus-security-ws.XXXXXX" ]]; then
  workspace="$("$REAL_MKTEMP" "$@")" || exit $?
  [[ -d "$workspace" ]] || exit 1
  printf '%s' "$workspace" > "$SECURITY_WS_CREATED"
  printf '%s\\n' "$workspace"
  exit 0
fi
exec "$REAL_MKTEMP" "$@"
`, { mode: 0o755 });
  // The broken-link probes run inside the created workspace; recording the
  // first one proves the script uses the directory mktemp actually created.
  // The doctor probe is the last command to run there: it must still find the
  // broken .locus.toml link, record the populated workspace, and fall through
  // to the real binary so its UNSAFE verdict is what the suite observes.
  const wrapper = join(bin, 'locus');
  writeFileSync(wrapper, `#!/bin/bash
if [[ "\${1:-}" == pin && "\${2:-}" == env-probe && "\${3:-}" == --force && \
  -f "$SECURITY_WS_CREATED" && "$PWD" == "$(cat "$SECURITY_WS_CREATED")" ]]; then
  [[ -L "$PWD/.locus.toml" ]] || exit 1
  printf '%s' "$PWD" > "$SECURITY_WS_ENTERED"
fi
if [[ "\${1:-}" == --json && "\${2:-}" == doctor && "$PWD" == "$PROBE_SCRATCH"/locus-security-ws.* ]]; then
  [[ -L .locus.toml ]] || exit 1
  printf '%s' "$PWD" > "$SECURITY_WS_USED"
  touch cleanup-sentinel || exit 1
fi
exec '${locus.replaceAll("'", "'\\''")}' "$@"
`, { mode: 0o755 });

  const result = spawnSync('bash', [script], {
    cwd: root,
    env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, TMPDIR: scratch,
      LOCUS_BIN: wrapper, REAL_MKTEMP: realMktemp, PROBE_SCRATCH: scratch,
      SECURITY_WS_CREATED: created, SECURITY_WS_TEMPLATES: templates,
      SECURITY_WS_ENTERED: entered, SECURITY_WS_USED: used },
    encoding: 'utf8',
    timeout: 1_800_000,
    maxBuffer: 10 * 1024 * 1024,
  });
  assert.equal(result.signal, null, 'e2e process terminated unexpectedly');
  assert.equal(result.status, 0, 'e2e should exit successfully');
  assert.match(result.stdout, /e2e PASS  \(\d+ checks, \d+ skipped\)/);
  assert.match(result.stdout, /ok  broken workspace link blocks force\/run and makes doctor UNSAFE/);
  assert.ok(existsSync(templates), 'no locus-security-ws template reached mktemp');
  // Sections 2c and 2e rehearse creation beneath their own nested spaced
  // TMPDIRs before the probes create the real fixture beneath the suite TMPDIR.
  const calls = readFileSync(templates, 'utf8').trim().split('\n');
  assert.equal(calls.length, 3, 'expected both subshell rehearsals and the probe creation');
  assert.ok(calls[0].startsWith(`${scratch}/locus-e2e.`),
    'subshell creation must sit beneath the suite home');
  assert.ok(calls[0].endsWith('/custom temp/locus-security-ws.XXXXXX'),
    'section 2c creation must use the nested TMPDIR');
  assert.ok(calls[1].startsWith(`${scratch}/locus-e2e.`),
    'section 2e creation must sit beneath the suite home');
  assert.ok(calls[1].endsWith('/security  ws temp root/locus-security-ws.XXXXXX'),
    'section 2e creation must use its nested TMPDIR');
  assert.equal(calls[2], `${scratch}/locus-security-ws.XXXXXX`);
  assert.ok(existsSync(created), 'security workspace mktemp did not run beneath TMPDIR');
  const workspace = readFileSync(created, 'utf8');
  assert.equal(dirname(workspace), scratch, 'created directly beneath the spaced TMPDIR');
  assert.match(workspace, /\/locus-security-ws\.[^/\s]+$/, 'workspace name is one unsplit component');
  assert.ok(existsSync(entered), 'the broken-link probe never ran inside the created workspace');
  assert.equal(readFileSync(entered, 'utf8'), workspace,
    'the script used a different workspace than mktemp created');
  assert.ok(existsSync(used), 'the broken-link doctor probe did not run inside the security workspace');
  assert.equal(readFileSync(used, 'utf8'), workspace,
    'security doctor used a different workspace');
  assert.equal(existsSync(workspace), false, 'security workspace survived exit cleanup');
  assert.throws(() => lstatSync(workspace), { code: 'ENOENT' },
    'security workspace and its contents must be removed on exit');
  assert.deepEqual(readdirSync(scratch), ['keep'], 'cleanup removed or left unexpected files');
});

test('populated security workspace is cleaned after a later probe failure', { timeout: 1_800_000 }, (t) => {
  const fixture = mkdtempSync(join(tmpdir(), 'locus-e2e-security-workspace-test-'));
  t.after(() => rmSync(fixture, { recursive: true, force: true }));
  const scratch = join(fixture, 'custom tmp $literal; [test]');
  const bin = join(fixture, 'bin');
  const record = join(fixture, 'workspace');
  mkdirSync(scratch);
  mkdirSync(bin);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  const realLn = execFileSync('which', ['ln'], { encoding: 'utf8' }).trim();
  // The script now links missing-policy.toml into SECURITY_WS twice: the
  // section 2e rehearsal does it beneath a nested TMPDIR re-exported inside
  // its subshell, and section 24 does it beneath the suite TMPDIR. Only the
  // section 24 call may trip this fixture, so the match is scoped to the
  // suite root (PROBE_SCRATCH) rather than the caller's TMPDIR — the same
  // discrimination the neighboring security-workspace path test uses.
  writeFileSync(join(bin, 'ln'), `#!/bin/bash
if [[ "$#" -eq 3 && "$1" == -s && "$2" == missing-policy.toml && \
      "$3" == "$PROBE_SCRATCH"/locus-security-ws.*/.locus.toml ]]; then
  workspace="\${3%/.locus.toml}"
  [[ -d "$workspace" ]] || exit 91
  "$REAL_LN" "$@" || exit 92
  [[ -L "$workspace/.locus.toml" ]] || exit 93
  touch "$workspace/cleanup-sentinel" || exit 94
  printf '%s' "$workspace" > "$WORKSPACE_RECORD"
  exit 73
fi
exec "$REAL_LN" "$@"
`, { mode: 0o755 });

  const result = spawnSync('bash', [script], {
    cwd: root,
    env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, TMPDIR: scratch,
      PROBE_SCRATCH: scratch, REAL_LN: realLn, WORKSPACE_RECORD: record },
    encoding: 'utf8',
    timeout: 1_800_000,
    maxBuffer: 10 * 1024 * 1024,
  });
  assert.equal(result.signal, null, 'e2e process terminated unexpectedly');
  assert.equal(result.status, 73, 'the injected failure status must survive cleanup');
  assert.match(result.stdout, /==> 24\. adversarial release credential\/workspace probes/);
  assert.doesNotMatch(result.stdout, /==> 25\./, 'the failure must stop the next section');
  const workspace = readFileSync(record, 'utf8');
  assert.equal(dirname(workspace), scratch);
  assert.match(workspace, /\/locus-security-ws\.[^/]+$/);
  assert.equal(existsSync(workspace), false, 'populated security workspace survived exit cleanup');
  assert.deepEqual(readdirSync(scratch), ['keep'], 'cleanup removed or left unexpected files');
});
