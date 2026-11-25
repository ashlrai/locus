// Run: node --test scripts/e2e-workspace-success-cleanup.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'e2e.sh');
const source = readFileSync(script, 'utf8');

function functionSource(name) {
  const match = source.match(new RegExp(`^${name}\\(\\) \\{[\\s\\S]*?^\\}`, 'm'));
  assert.ok(match, `missing ${name} function`);
  return match[0];
}

test('successful stubbed workspace rehearsal removes its populated workspace', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-ws-success-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const home = join(root, 'main home');
  const stub = join(root, 'locus');
  const record = join(root, 'workspace-used');
  mkdirSync(home);
  writeFileSync(join(home, 'keep'), 'unrelated');
  writeFileSync(stub, `#!/bin/bash
case "$*" in
  init) [[ -d "$LOCUS_HOME" ]] ;;
  'workspace --default acme --allow acme')
    printf 'allowlist\n' > .locus.toml
    printf '%s' "$PWD" > "$WORKSPACE_RECORD" ;;
  *) exit 1 ;;
esac
`, { mode: 0o755 });

  const start = source.indexOf('log "2d. workspace creation honors');
  const end = source.indexOf('# A plain custom TMPDIR', start);
  assert.ok(start >= 0 && end > start, 'missing workspace rehearsal');
  const result = spawnSync('bash', ['-c', `set -euo pipefail
log() { :; }
ok() { :; }
die() { printf '%s\\n' "$*" >&2; exit 1; }
${functionSource('create_workspace')}
${functionSource('e2e_cleanup')}
${source.slice(start, end)}
`], {
    env: { ...process.env, LOCUS_HOME: home, LOCUS_BIN: stub, WORKSPACE_RECORD: record },
    encoding: 'utf8',
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, result.stderr);
  const workspace = readFileSync(record, 'utf8');
  assert.equal(workspace, readFileSync(join(home, 'workspace  ws temp root', 'workspace-path'), 'utf8'));
  assert.equal(existsSync(workspace), false, 'the populated workspace must be removed on success');
  assert.equal(readFileSync(join(home, 'keep'), 'utf8'), 'unrelated');
  assert.ok(existsSync(join(home, 'workspace', 'decoy-sentinel')));
});

test('offline e2e command failure removes its populated workspace', { timeout: 15_000 }, (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-ws-failure-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'scratch with spaces');
  const stub = join(root, 'locus');
  const hook = join(root, 'hook');
  const record = join(root, 'workspace-used');
  const cleaned = join(root, 'workspace-cleaned');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  writeFileSync(stub, `#!/bin/bash
set -euo pipefail
case "$*" in
  init) touch "$LOCUS_HOME/initialized" ;;
  'workspace --default acme --allow acme')
    [[ -d "$PWD" && -f "$LOCUS_HOME/initialized" ]]
    printf 'allowlist\\n' > .locus.toml
    printf '%s' "$PWD" > "$WORKSPACE_RECORD"
    exit 73 ;;
  *) exit 74 ;;
esac
`, { mode: 0o755 });
  // Stub the build, so this executes the actual script without Rust or network
  // access. Observe workspace removal separately: removing the outer home
  // would otherwise hide a missing cleanup trap in the rehearsal subshell.
  writeFileSync(hook, `cargo() { :; }
rm() {
  local workspace="" arg matched=0
  if [[ -f "$WORKSPACE_RECORD" ]]; then
    workspace="$(cat "$WORKSPACE_RECORD")"
    for arg in "$@"; do
      if [[ "$arg" == "$workspace" ]]; then
        [[ -s "$workspace/.locus.toml" ]] || return 75
        matched=1
      fi
    done
  fi
  command rm "$@" || return $?
  if [[ "$matched" == 1 ]]; then
    [[ ! -e "$workspace" && ! -L "$workspace" ]] || return 76
    printf '%s' "$workspace" > "$WORKSPACE_CLEANED"
  fi
}
`);
  const result = spawnSync('bash', [script], {
    env: { ...process.env, BASH_ENV: hook, TMPDIR: scratch,
      LOCUS_BIN: stub, MCP_BIN: stub, WORKSPACE_RECORD: record, WORKSPACE_CLEANED: cleaned },
    encoding: 'utf8',
    timeout: 10_000,
  });

  assert.ifError(result.error);
  assert.equal(result.signal, null);
  assert.equal(result.status, 1, 'the failed rehearsal must stop the full script');
  assert.match(result.stderr, /FAIL workspace was not created beneath the space-containing TMPDIR/);
  assert.doesNotMatch(result.stdout, /==> 2d2\./, 'execution must stop after the command failure');
  const workspace = readFileSync(record, 'utf8');
  const home = dirname(dirname(workspace));
  assert.equal(dirname(home), scratch, 'workspace must belong to this run');
  assert.match(workspace, /\/workspace  ws temp root\/locus-ws\.[^/]+$/);
  assert.equal(readFileSync(cleaned, 'utf8'), workspace, 'the workspace trap must remove the populated directory');
  assert.equal(existsSync(workspace), false, 'workspace survived failure cleanup');
  assert.equal(existsSync(home), false, 'outer home survived failure cleanup');
  assert.deepEqual(readdirSync(scratch), ['keep'], 'cleanup must spare unrelated files');
  assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'unrelated');
});
