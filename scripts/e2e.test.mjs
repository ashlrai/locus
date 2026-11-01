// Run: node --test scripts/e2e.test.mjs
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const script = join(root, 'scripts', 'e2e.sh');
const locus = join(root, 'target', 'release', 'locus');

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
  writeFileSync(join(bin, 'mktemp'), `#!/bin/bash
case "$*" in
  *locus-e2e.XXXXXX*)
    [[ $# -eq 2 && "$1" == -d && "$2" == "$TMPDIR/locus-e2e.XXXXXX" ]] || exit 1
    home="$("$REAL_MKTEMP" "$@")" || exit $?
    printf '%s' "$home" > "$HOME_CREATED"
    printf '%s\\n' "$home"
    ;;
  *locus-ws.XXXXXX*)
    [[ $# -eq 2 && "$1" == -d && "$2" == "$TMPDIR/locus-ws.XXXXXX" ]] || exit 1
    workspace="$("$REAL_MKTEMP" "$@")" || exit $?
    printf '%s' "$workspace" > "$WORKSPACE_CREATED"
    printf '%s\\n' "$workspace"
    ;;
  *) exec "$REAL_MKTEMP" "$@" ;;
esac
`, { mode: 0o755 });
  const wrapper = join(bin, 'locus');
  writeFileSync(wrapper, `#!/bin/bash
if [[ "\${1:-}" == workspace ]]; then
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
    const cleanedEarly = join(fixture, 'cleaned-before-next-section');
    mkdirSync(scratch);
    mkdirSync(bin);
    writeFileSync(join(scratch, 'keep'), 'unrelated');

    // Force both same-home attempts to fail, then observe the real fresh-home import.
    const wrapper = join(bin, 'locus');
    writeFileSync(wrapper, `#!/bin/bash
if [[ "\${1:-}" == init && "$LOCUS_HOME" == "$TMPDIR"/locus-graph-imp.* ]]; then
  [[ -d "$LOCUS_HOME" && "$LOCUS_GRAPH_PASSPHRASE" == e2e-test ]] || exit 1
  printf '%s' "$LOCUS_HOME" > "$GRAPH_INIT_RECORD"
  [[ "$GRAPH_FAILURE" != init ]] || exit 1
fi
if [[ "\${1:-}" == graph && "\${2:-}" == import ]]; then
  if [[ "$LOCUS_HOME" != "$TMPDIR"/locus-graph-imp.* ]]; then
    exit 1
  fi
  [[ -d "$LOCUS_HOME" && "$LOCUS_GRAPH_PASSPHRASE" == e2e-test ]] || exit 1
  [[ "$(cat "$GRAPH_INIT_RECORD")" == "$LOCUS_HOME" ]] || exit 1
  printf '%s' "$LOCUS_HOME" > "$GRAPH_HOME_RECORD"
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
        LOCUS_BIN: wrapper,
        GRAPH_HOME_RECORD: observed,
        GRAPH_IMPORT_RECORD: imported,
        GRAPH_INIT_RECORD: initialized,
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
    assert.equal(existsSync(imported), failure !== 'init', 'unexpected fresh graph import');
    const graphHome = readFileSync(initialized, 'utf8');
    if (failure !== 'init') assert.equal(readFileSync(observed, 'utf8'), graphHome);
    assert.ok(dirname(graphHome) === scratch, 'graph home was not created directly under TMPDIR');
    assert.ok(/\/locus-graph-imp\.[^/]+$/.test(graphHome), 'unexpected graph home name');
    assert.equal(existsSync(graphHome), false, 'graph home was not cleaned up');
    assert.deepEqual(readdirSync(scratch), ['keep'], 'cleanup removed or left unexpected files');
    if (failure !== 'none') {
      assert.ok(/graph import failed/.test(result.stderr), 'expected graph import failure');
    } else {
      assert.ok(existsSync(cleanedEarly), 'graph home survived until the next section');
      assert.match(result.stdout, /ok  sample exec succeeds without unset-variable warnings/);
      assert.match(result.stdout, /ok  sample exec reports missing credentials/);
      assert.ok(/ok  graph export\/import into fresh LOCUS_HOME/.test(result.stdout), 'fresh import did not pass');
      assert.ok(/e2e PASS/.test(result.stdout), 'e2e did not pass');
      const summary = result.stdout.match(/e2e PASS  \((\d+) checks, (\d+) skipped\)/);
      assert.ok(summary, 'e2e summary missing');
      t.diagnostic(`e2e: ${summary[1]} checks passed, ${summary[2]} skipped`);
    }
  });
}

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
