// Run: node --test scripts/hub-integration-test.test.mjs
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-integration-test.sh');
const realMktemp = execFileSync('which', ['mktemp'], { encoding: 'utf8' }).trim();

function runWithTempDir(t, failProject = false, scratchParts = ['custom tmp']) {
  // Keep paths short enough for the live authority broker's Unix socket.
  const root = mkdtempSync('/tmp/lhi-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const bin = join(root, 'bin');
  const scratch = join(root, ...scratchParts);
  const record = join(root, 'created');
  mkdirSync(bin);
  mkdirSync(scratch, { recursive: true });
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

test('integration smoke handles spaces in temporary home and project paths', (t) => {
  const { result, scratch, paths } = runWithTempDir(t, false, ['a b', 'c d']);
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
  assert.match(result.stdout, /^== hub-integration-test OK ==$/m);
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
