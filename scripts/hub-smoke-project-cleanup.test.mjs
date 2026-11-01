// Run: node --test scripts/hub-smoke-project-cleanup.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-smoke.sh');

test('normal completion removes the populated temporary project', (t) => {
  // Keep the path short enough for the authority broker's Unix socket.
  const root = mkdtempSync('/tmp/locus-proj-test-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const projectRecord = join(root, 'project');
  const observed = join(root, 'observed');
  const hook = join(root, 'hook.sh');
  writeFileSync(hook, `mktemp() {
  local path
  path="$(command mktemp "$@")" || return
  case "$path" in
    "$TMPDIR"/locus-hub-proj.*) printf '%s' "$path" > "$PROJECT_RECORD" ;;
  esac
  printf '%s\\n' "$path"
}
rm() {
  local project
  project="$(cat "$PROJECT_RECORD")"
  if [[ "$PWD" == "$project" && -f "$project/.mcp.json" ]]; then
    printf '%s' observed > "$PROJECT_OBSERVED"
  fi
  command rm "$@"
}
`);
  const result = spawnSync('bash', [script], {
    env: { ...process.env, TMPDIR: root, BASH_ENV: hook,
      PROJECT_RECORD: projectRecord, PROJECT_OBSERVED: observed },
    encoding: 'utf8',
    maxBuffer: 4 * 1024 * 1024,
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 0, 'smoke must complete successfully');
  assert.ok(result.stdout.split('\n').includes('== hub-smoke OK =='), 'success banner missing');
  assert.equal(readFileSync(observed, 'utf8'), 'observed', 'cleanup must see the populated project');
  const project = readFileSync(projectRecord, 'utf8');
  assert.equal(dirname(project), root);
  assert.equal(existsSync(project), false, 'temporary project must be removed');
});
