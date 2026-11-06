// Run: node --test scripts/hub-smoke-project-failure.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-smoke.sh');
const registration = { mcpServers: { locus: { command: 'locus-mcp', args: [] } } };

// The mktemp shim records the allocated project path; the rm shim captures the
// project registration at removal time, while the trap still runs from inside
// the project directory the script cd'd into.
const shims = `mktemp() {
  local path
  path="$(command mktemp "$@")" || return
  case "$path" in
    "$TMPDIR"/locus-hub-proj.*) printf '%s\\n' "$path" > "$PROJECT_RECORD" ;;
  esac
  printf '%s\\n' "$path"
}
jq() { cat >/dev/null; printf '%s\\n' grant; }
cargo() { return 0; }
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
`;

test('a failed check removes the populated project directory', (t) => {
  const root = mkdtempSync('/tmp/locus-proj-fail-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'temporary resources with spaces');
  const projectRecord = join(root, 'project');
  const observed = join(root, 'observed');
  const hook = join(root, 'hook.sh');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'locus-hub-proj.unrelated'), 'keep');
  // Init/pin and every check_json pass, but status --oneline emits a value
  // without the alias:tenant colon, so the run finishes with fail=1.
  writeFileSync(hook, `${shims}locus() {
  if [[ "\${1:-} \${2:-}" == 'status --oneline' ]]; then
    printf '%s\\n' missing-colon
    return 0
  fi
  printf '%s\\n' '{}'
}
`);
  const result = spawnSync('bash', [script], {
    env: { ...process.env, TMPDIR: scratch, BASH_ENV: hook,
      PROJECT_RECORD: projectRecord, PROJECT_OBSERVED: observed },
    encoding: 'utf8',
    timeout: 15_000,
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 1);
  assert.ok(result.stdout.includes('FAIL  status --oneline expected alias:tenant, got: missing-colon'));
  // The remaining checks still run before the failure banner.
  assert.ok(result.stdout.includes('ok    mcp revoke removes grant from roster'));
  assert.ok(result.stdout.split('\n').includes('== hub-smoke FAILED =='), 'failure banner missing');
  assert.doesNotMatch(result.stdout, /^skip\s/m, 'all smoke checks must run');
  assert.deepEqual(JSON.parse(readFileSync(observed, 'utf8')), registration,
    'cleanup must remove the project while its MCP registration is still inside');
  const project = readFileSync(projectRecord, 'utf8').trim();
  assert.equal(dirname(project), scratch);
  assert.ok(project.slice(scratch.length + 1).startsWith('locus-hub-proj.'));
  assert.equal(existsSync(project), false, 'the populated project directory must be removed');
  assert.deepEqual(readdirSync(scratch), ['locus-hub-proj.unrelated']);
  assert.equal(readFileSync(join(scratch, 'locus-hub-proj.unrelated'), 'utf8'), 'keep');
});

test('an abrupt command failure removes the populated project directory', (t) => {
  const root = mkdtempSync('/tmp/locus-proj-fail-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'temporary resources with spaces');
  const projectRecord = join(root, 'project');
  const observed = join(root, 'observed');
  const hook = join(root, 'hook.sh');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'locus-hub-proj.unrelated'), 'keep');
  // status --oneline exits nonzero mid-checks, after cd and the .mcp.json
  // write; set -e aborts the script without a final banner.
  writeFileSync(hook, `${shims}locus() {
  if [[ "\${1:-} \${2:-}" == 'status --oneline' ]]; then
    printf 'injected status failure\\n' >&2
    return 1
  fi
  printf '%s\\n' '{}'
}
`);
  const result = spawnSync('bash', [script], {
    env: { ...process.env, TMPDIR: scratch, BASH_ENV: hook,
      PROJECT_RECORD: projectRecord, PROJECT_OBSERVED: observed },
    encoding: 'utf8',
    timeout: 15_000,
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /injected status failure/);
  // Checks before the abort printed normally; no final banner is reached.
  assert.ok(result.stdout.includes('ok    agent report --json'));
  assert.ok(result.stdout.includes('ok    status --json'));
  assert.doesNotMatch(result.stdout, /== hub-smoke (OK|FAILED) ==/);
  assert.deepEqual(JSON.parse(readFileSync(observed, 'utf8')), registration,
    'cleanup must remove the project while its MCP registration is still inside');
  const project = readFileSync(projectRecord, 'utf8').trim();
  assert.equal(dirname(project), scratch);
  assert.ok(project.slice(scratch.length + 1).startsWith('locus-hub-proj.'));
  assert.equal(existsSync(project), false, 'the populated project directory must be removed');
  assert.deepEqual(readdirSync(scratch), ['locus-hub-proj.unrelated']);
  assert.equal(readFileSync(join(scratch, 'locus-hub-proj.unrelated'), 'utf8'), 'keep');
});
