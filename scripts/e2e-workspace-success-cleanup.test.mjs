// Run: node --test scripts/e2e-workspace-success-cleanup.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const source = readFileSync(join(dirname(fileURLToPath(import.meta.url)), 'e2e.sh'), 'utf8');

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
