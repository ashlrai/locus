// Run: node --test scripts/hub-smoke.test.mjs
import assert from 'node:assert/strict';
import { execFileSync, spawn, spawnSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { constants, tmpdir } from 'node:os';
import { basename, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-smoke.sh');
const realMktemp = execFileSync('which', ['mktemp'], { encoding: 'utf8' }).trim();

test('invalid TMPDIR fails clearly before starting the smoke test', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'hub-smoke-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const result = spawnSync('bash', [script], {
    env: { ...process.env, TMPDIR: join(root, 'missing', 'nested') },
    encoding: 'utf8',
  });

  assert.notEqual(result.status, 0);
  assert.equal(result.signal, null);
  assert.match(result.stderr, /error: hub-smoke could not create temporary home/);
  assert.equal(result.stdout, '');
  assert.deepEqual(readdirSync(root), []);
});

for (const [template, message] of [
  ['locus-hub-smoke.XXXXXX', 'temporary home'],
  ['locus-hub-proj.XXXXXX', 'temporary project directory'],
  ['locus-hub-smoke-err.XXXXXX', 'temporary error file'],
]) {
  test(`failure creating ${message} stops and cleans up`, (t) => {
    const root = mkdtempSync(join(tmpdir(), 'hub-smoke-test-'));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const bin = join(root, 'bin');
    const scratch = join(root, 'scratch');
    mkdirSync(bin);
    mkdirSync(scratch);
    writeFileSync(join(bin, 'mktemp'), `#!/usr/bin/env bash
for arg in "$@"; do
  if [[ "$arg" == */"$FAIL_TEMPLATE" ]]; then exit 1; fi
done
exec "$REAL_MKTEMP" "$@"
`, { mode: 0o755 });
    const result = spawnSync('bash', [script], {
      env: { ...process.env, TMPDIR: scratch, PATH: `${bin}:${process.env.PATH}`, FAIL_TEMPLATE: template, REAL_MKTEMP: realMktemp },
      encoding: 'utf8',
    });

    assert.notEqual(result.status, 0);
    assert.equal(result.signal, null);
    assert.match(result.stderr, new RegExp(`error: hub-smoke could not create ${message}`));
    assert.equal(result.stdout, '');
    assert.deepEqual(readdirSync(scratch), []);
  });
}

function cleanupFixture(t, interrupt = false) {
  const root = mkdtempSync(join(tmpdir(), 'hub-smoke-cleanup-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scripts = join(root, 'scripts');
  const bin = join(root, 'target', 'debug');
  const scratch = join(root, 'scratch with spaces');
  const ready = join(root, 'ready');
  mkdirSync(scripts);
  mkdirSync(bin, { recursive: true });
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  copyFileSync(script, join(scripts, 'hub-smoke.sh'));
  writeFileSync(join(bin, 'jq'), '#!/bin/sh\ncat >/dev/null\nexit 0\n', { mode: 0o755 });
  writeFileSync(join(bin, 'locus'), `#!/usr/bin/env bash
if [[ "${'$'}1" == init ]]; then
  files=("${'$'}TMPDIR"/locus-hub-smoke-err.*)
  [[ "${'$'}{#files[@]}" == 1 && -f "${'$'}{files[0]}" ]] || exit 1
  printf '%s' "${'$'}{files[0]}" > "${'$'}PROBE_READY"
  if [[ "${'$'}{PROBE_INTERRUPT:-}" == 1 ]]; then
    while :; do sleep 1; done
  fi
fi
if [[ "${'$'}1" == status && "${'$'}{2:-}" == --oneline ]]; then
  echo 'personal:fixture'
else
  echo '{}'
fi
`, { mode: 0o755 });
  for (const path of ['schema/agent-report.schema.json', 'schema/doctor.schema.json',
    'schema/hub-gate.schema.json', 'schema/mcp-grant.schema.json', 'docs/hub-integration.md',
    'integrations/phantom/README.md', 'integrations/phantom/fleet-preflight.md']) {
    mkdirSync(dirname(join(root, path)), { recursive: true });
    writeFileSync(join(root, path), 'fixture');
  }
  return {
    script: join(scripts, 'hub-smoke.sh'), scratch, ready,
    env: { HOME: root, PATH: '/usr/bin:/bin', TMPDIR: scratch, PROBE_READY: ready,
      PROBE_INTERRUPT: interrupt ? '1' : '0' },
  };
}

function assertCleaned(fixture) {
  const errorFile = readFileSync(fixture.ready, 'utf8');
  assert.equal(dirname(errorFile), fixture.scratch);
  assert.match(basename(errorFile), /^locus-hub-smoke-err\./);
  assert.equal(existsSync(errorFile), false, 'the allocated error file must be removed');
  assert.deepEqual(readdirSync(fixture.scratch), ['keep']);
}

test('successful smoke cleans its allocated error file', (t) => {
  const fixture = cleanupFixture(t);
  const result = spawnSync('bash', [fixture.script], { env: fixture.env, encoding: 'utf8', timeout: 10000 });
  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 0, 'fixture smoke must complete successfully');
  assert.doesNotMatch(result.stdout, /^skip\s/m);
  assertCleaned(fixture);
});

for (const signal of ['SIGINT', 'SIGTERM']) {
  test(`${signal} interrupted smoke cleans its allocated error file`, { timeout: 10000 }, async (t) => {
    const fixture = cleanupFixture(t, true);
    const child = spawn('bash', [fixture.script], { env: fixture.env, detached: true, stdio: 'ignore' });
    let exited = false;
    const closed = new Promise((resolve, reject) => {
      child.once('error', reject);
      child.once('close', (code, signal) => { exited = true; resolve({ code, signal }); });
    });
    t.after(() => {
      if (!exited && child.pid) {
        try { process.kill(-child.pid, 'SIGKILL'); } catch (error) {
          if (error.code !== 'ESRCH') throw error;
        }
      }
    });
    const deadline = Date.now() + 5000;
    while (!existsSync(fixture.ready) && !exited && Date.now() < deadline) {
      await new Promise((resolve) => setTimeout(resolve, 20));
    }
    assert.equal(existsSync(fixture.ready), true, 'smoke must reach init with the error file present');
    assert.equal(exited, false, 'smoke must still be running before interruption');
    assert.equal(existsSync(readFileSync(fixture.ready, 'utf8')), true, 'error file must exist before interruption');
    process.kill(-child.pid, signal);
    const result = await closed;
    assert.ok(result.signal === signal || result.code === 128 + constants.signals[signal],
      `smoke must terminate due to ${signal}`);
    assertCleaned(fixture);
  });
}
