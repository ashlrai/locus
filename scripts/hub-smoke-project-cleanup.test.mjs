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
  const sentinel = join(root, 'keep');
  writeFileSync(sentinel, 'unrelated');
  writeFileSync(hook, `mktemp() {
  local path
  path="$(command mktemp "$@")" || return
  case "$path" in
    "$TMPDIR"/locus-hub-proj.*) printf '%s' "$path" > "$PROJECT_RECORD" ;;
  esac
  printf '%s\\n' "$path"
}
rm() {
  local project arg targeted=
  project="$(cat "$PROJECT_RECORD")"
  for arg in "$@"; do
    [[ "$arg" == "$project" ]] && targeted=1
  done
  if [[ -n "$targeted" && "$PWD" == "$project" && -f "$project/.mcp.json" ]]; then
    cat "$project/.mcp.json" > "$PROJECT_OBSERVED"
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
  assert.doesNotMatch(result.stdout, /^skip\s/m, 'all smoke checks must run');
  assert.deepEqual(JSON.parse(readFileSync(observed, 'utf8')), {
    mcpServers: { locus: { command: 'locus-mcp', args: [] } },
  }, 'cleanup must target the project containing its MCP registration');
  const project = readFileSync(projectRecord, 'utf8');
  assert.equal(dirname(project), root);
  assert.ok(project.slice(root.length + 1).startsWith('locus-hub-proj.'));
  assert.equal(existsSync(project), false, 'temporary project must be removed');
  assert.equal(readFileSync(sentinel, 'utf8'), 'unrelated');
});
