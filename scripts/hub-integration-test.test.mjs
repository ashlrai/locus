// Run: node --test scripts/hub-integration-test.test.mjs
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-integration-test.sh');
const realMktemp = execFileSync('which', ['mktemp'], { encoding: 'utf8' }).trim();

function runWithTempDir(t, failProject = false) {
  // Keep paths short enough for the live authority broker's Unix socket.
  const root = mkdtempSync('/tmp/lhi-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const bin = join(root, 'bin');
  const scratch = join(root, 'custom tmp');
  const record = join(root, 'created');
  mkdirSync(bin);
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'sentinel');
  writeFileSync(join(bin, 'mktemp'), `#!/usr/bin/env bash
if [[ "${'$'}{FAIL_PROJECT:-}" == 1 && "${'$'}{*: -1}" == */locus-hub-int-proj.XXXXXX ]]; then exit 1; fi
path="$("${'$'}REAL_MKTEMP" "${'$'}@")" || exit
[[ -d "${'$'}path" ]] || exit 1
printf '%s\\n' "${'$'}path" >> "${'$'}MKTEMP_RECORD"
printf '%s\\n' "${'$'}path"
`, { mode: 0o755 });
  const result = spawnSync('bash', [script], {
    env: { ...process.env, TMPDIR: scratch, PATH: `${bin}:${process.env.PATH}`,
      REAL_MKTEMP: realMktemp, MKTEMP_RECORD: record, FAIL_PROJECT: failProject ? '1' : '0' },
    encoding: 'utf8',
    maxBuffer: 4 * 1024 * 1024,
  });
  const paths = readFileSync(record, 'utf8').trim().split('\n');
  return { result, scratch, paths };
}

test('integration smoke uses and cleans a nondefault TMPDIR', (t) => {
  const { result, scratch, paths } = runWithTempDir(t);
  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 0, 'integration smoke must succeed');
  assert.doesNotMatch(result.stdout, /^skip\s/m, 'all integration checks must run');
  assert.equal(paths.length, 2);
  assert.match(paths[0], new RegExp(`^${scratch}/locus-hub-int\\.[^/]+$`));
  assert.match(paths[1], new RegExp(`^${scratch}/locus-hub-int-proj\\.[^/]+$`));
  assert.deepEqual(readdirSync(scratch), ['keep']);
});

test('failed project allocation cleans the temporary home', (t) => {
  const { result, scratch, paths } = runWithTempDir(t, true);
  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.notEqual(result.status, 0);
  assert.equal(paths.length, 1);
  assert.match(paths[0], new RegExp(`^${scratch}/locus-hub-int\\.[^/]+$`));
  assert.deepEqual(readdirSync(scratch), ['keep']);
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
