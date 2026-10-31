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

test('workspace creation preserves a spaced TMPDIR argument and cleans up on failure', { timeout: 1_800_000 }, (t) => {
  const fixture = mkdtempSync(join(tmpdir(), 'locus-e2e-workspace-test-'));
  t.after(() => rmSync(fixture, { recursive: true, force: true }));
  const scratch = join(fixture, 'custom tmp with spaces');
  const bin = join(fixture, 'bin');
  const created = join(fixture, 'workspace-created');
  const entered = join(fixture, 'workspace-entered');
  mkdirSync(scratch);
  mkdirSync(bin);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  const realMktemp = execFileSync('which', ['mktemp'], { encoding: 'utf8' }).trim();
  writeFileSync(join(bin, 'mktemp'), `#!/bin/bash
case "$*" in
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
  printf '%s' "$PWD" > "$WORKSPACE_ENTERED"
  touch "$PWD/cleanup-sentinel"
  exit 1
fi
exec '${locus.replaceAll("'", "'\\''")}' "$@"
`, { mode: 0o755 });

  const result = spawnSync('bash', [script], {
    cwd: root,
    env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, TMPDIR: scratch,
      LOCUS_BIN: wrapper, REAL_MKTEMP: realMktemp, WORKSPACE_CREATED: created,
      WORKSPACE_ENTERED: entered },
    encoding: 'utf8',
    timeout: 1_800_000,
    maxBuffer: 10 * 1024 * 1024,
  });
  assert.equal(result.signal, null, 'e2e process terminated unexpectedly');
  assert.equal(result.status, 1, 'the injected workspace failure should stop e2e');
  assert.match(result.stdout, /==> 4\. workspace allowlist/);
  assert.ok(existsSync(created), 'workspace mktemp did not receive the expected arguments');
  assert.ok(existsSync(entered), 'the workspace command was not reached');
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

for (const failImport of [false, true]) {
  test(`graph fallback uses TMPDIR and cleans up ${failImport ? 'after import failure' : 'after import'}`, { timeout: 1_800_000 }, (t) => {
    const fixture = mkdtempSync(join(tmpdir(), 'locus-e2e-test-'));
    t.after(() => rmSync(fixture, { recursive: true, force: true }));
    const scratch = join(fixture, 'custom tmp');
    const bin = join(fixture, 'bin');
    const observed = join(fixture, 'graph-home');
    const imported = join(fixture, 'import-observed');
    mkdirSync(scratch);
    mkdirSync(bin);

    // Force both same-home attempts to fail, then observe the real fresh-home import.
    const wrapper = join(bin, 'locus');
    writeFileSync(wrapper, `#!/bin/bash
if [[ "\${1:-}" == graph && "\${2:-}" == import ]]; then
  if [[ "$LOCUS_HOME" != "$TMPDIR"/locus-graph-imp.* ]]; then
    exit 1
  fi
  [[ -d "$LOCUS_HOME" ]] || exit 1
  printf '%s' "$LOCUS_HOME" > "$GRAPH_HOME_RECORD"
  : > "$GRAPH_IMPORT_RECORD"
  [[ "$FAIL_GRAPH_IMPORT" != 1 ]] || exit 1
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
        FAIL_GRAPH_IMPORT: failImport ? '1' : '0',
        LOCUS_GRAPH_PASSPHRASE: 'e2e-test',
      },
      encoding: 'utf8',
      timeout: 1_800_000,
      maxBuffer: 10 * 1024 * 1024,
    });

    assert.equal(result.signal, null, 'e2e process terminated unexpectedly');
    const stages = [...result.stdout.matchAll(/^==> (\d+[a-z]?)\./gm)];
    assert.equal(result.status, failImport ? 1 : 0,
      `unexpected e2e exit status at stage ${stages.at(-1)?.[1] ?? 'before startup'}`);
    assert.ok(existsSync(imported), 'fresh graph import was not reached');
    const graphHome = readFileSync(observed, 'utf8');
    assert.ok(dirname(graphHome) === scratch, 'graph home was not created directly under TMPDIR');
    assert.ok(/\/locus-graph-imp\.[^/]+$/.test(graphHome), 'unexpected graph home name');
    assert.equal(existsSync(graphHome), false, 'graph home was not cleaned up');
    assert.deepEqual(readdirSync(scratch), [], 'e2e left temporary directories behind');
    if (failImport) {
      assert.ok(/graph import failed/.test(result.stderr), 'expected graph import failure');
    } else {
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
