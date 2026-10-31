// Run: node --test scripts/hub-smoke.test.mjs
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-smoke.sh');
const realMktemp = execFileSync('which', ['mktemp'], { encoding: 'utf8' }).trim();

test('header documents the local smoke command and prerequisites', () => {
  const header = readFileSync(script, 'utf8').split('set -euo pipefail', 1)[0];
  assert.match(header, /repository root[^\n]*\.\/scripts\/hub-smoke\.sh/);
  for (const prerequisite of ['Bash', 'jq', 'Rust', 'Cargo', 'mktemp', '/dev/urandom']) {
    assert.ok(header.includes(prerequisite), `missing prerequisite: ${prerequisite}`);
  }
  assert.match(header, /writable temporary storage and target\//);
  assert.match(header, /throwaway LOCUS_HOME/);
});

test('prerequisites document tracks the script environment and checked files', () => {
  const source = readFileSync(script, 'utf8');
  const doc = readFileSync(join(dirname(script), '../docs/hub-smoke-prerequisites.md'), 'utf8');
  for (const variable of ['HOME', 'PATH', 'TMPDIR', 'LOCUS_CONTROL_CAPABILITY',
    'LOCUS_AUTHORITY_BROKER_START_TIMEOUT_MS', 'LOCUS_HOME', 'LOCUS_SESSION_ID']) {
    assert.ok(source.includes(variable) && doc.includes(`\`${variable}\``), `missing environment variable: ${variable}`);
  }
  for (const match of source.matchAll(/for f in ([^;]+); do/g)) {
    for (const file of match[1].trim().split(/\s+/)) {
      const path = file.endsWith('.schema.json') ? `schema/${file}` : file;
      assert.ok(doc.includes(path), `missing checked file: ${path}`);
    }
  }
  assert.match(doc, /overall smoke may still succeed/);
  assert.match(doc, /valid JSON response can pass even if/);
  assert.match(doc, /== hub-smoke OK ==/);
});

function smokeCommandGuard(root) {
  const guard = join(root, 'guard.sh');
  const marker = join(root, 'smoke-command-ran');
  // Functions also intercept binaries in the script's prepended target directories.
  writeFileSync(guard, `locus() { : > "$SMOKE_COMMAND_MARKER"; return 1; }
cargo() { : > "$SMOKE_COMMAND_MARKER"; return 1; }
jq() { : > "$SMOKE_COMMAND_MARKER"; return 1; }
`);
  return { BASH_ENV: guard, SMOKE_COMMAND_MARKER: marker };
}

test('successful smoke cleans all resources under a custom TMPDIR', (t) => {
  // The authority broker uses a Unix socket, so keep the temporary path short.
  const root = mkdtempSync('/tmp/lhs-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const bin = join(root, 'bin');
  const scratch = join(root, 'custom tmp');
  const record = join(root, 'created');
  mkdirSync(bin);
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'sentinel');
  writeFileSync(join(bin, 'mktemp'), `#!/usr/bin/env bash
path="$("$REAL_MKTEMP" "$@")" || exit
case "$path" in
  "$TMPDIR"/locus-hub-smoke.*|"$TMPDIR"/locus-hub-proj.*|"$TMPDIR"/locus-hub-smoke-err.*)
    printf '%s\\n' "$path" >> "$MKTEMP_RECORD" ;;
esac
printf '%s\\n' "$path"
`, { mode: 0o755 });
  const env = { ...process.env, TMPDIR: scratch, PATH: `${bin}:${process.env.PATH}`,
    REAL_MKTEMP: realMktemp, MKTEMP_RECORD: record };
  delete env.BASH_ENV;
  const result = spawnSync('bash', [script], {
    env,
    encoding: 'utf8',
    maxBuffer: 4 * 1024 * 1024,
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 0, 'smoke must succeed');
  assert.match(result.stdout, /^== hub-smoke OK ==$/m);
  assert.doesNotMatch(result.stdout, /^skip\s/m, 'all smoke checks must run');
  const paths = readFileSync(record, 'utf8').trim().split('\n');
  assert.equal(paths.length, 3);
  for (const [path, prefix] of paths.map((path, i) => [path, [
    'locus-hub-smoke.', 'locus-hub-proj.', 'locus-hub-smoke-err.',
  ][i]])) {
    assert.equal(dirname(path), scratch);
    assert.ok(path.slice(scratch.length + 1).startsWith(prefix));
    assert.equal(existsSync(path), false, `${prefix} must be removed`);
  }
  assert.deepEqual(readdirSync(scratch), ['keep']);
  assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'sentinel');
});

for (const invalidDir of ['missing', 'file']) {
  test(`${invalidDir} TMPDIR fails clearly before starting the smoke test`, (t) => {
    const root = mkdtempSync(join(tmpdir(), 'hub-smoke-test-'));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const guard = smokeCommandGuard(root);
    const invalidPath = invalidDir === 'missing' ? join(root, 'missing', 'nested') : join(root, 'file');
    if (invalidDir === 'file') writeFileSync(invalidPath, 'keep');
    const result = spawnSync('bash', [script], {
      env: { ...process.env, ...guard, TMPDIR: invalidPath },
      encoding: 'utf8',
    });

    assert.equal(result.error, undefined);
    assert.ok(Number.isInteger(result.status) && result.status !== 0);
    assert.equal(result.signal, null);
    assert.match(result.stderr, /error: hub-smoke could not create temporary home/);
    assert.equal(result.stdout, '');
    assert.equal(existsSync(guard.SMOKE_COMMAND_MARKER), false);
    assert.deepEqual(readdirSync(root).sort(), invalidDir === 'file' ? ['file', 'guard.sh'] : ['guard.sh']);
  });
}

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
    const guard = smokeCommandGuard(root);
    writeFileSync(join(bin, 'mktemp'), `#!/usr/bin/env bash
for arg in "$@"; do
  if [[ "$arg" == */"$FAIL_TEMPLATE" ]]; then exit 1; fi
done
exec "$REAL_MKTEMP" "$@"
`, { mode: 0o755 });
    const result = spawnSync('bash', [script], {
      env: { ...process.env, ...guard, TMPDIR: scratch, PATH: `${bin}:${process.env.PATH}`, FAIL_TEMPLATE: template, REAL_MKTEMP: realMktemp },
      encoding: 'utf8',
    });

    assert.equal(result.error, undefined);
    assert.ok(Number.isInteger(result.status) && result.status !== 0);
    assert.equal(result.signal, null);
    assert.match(result.stderr, new RegExp(`error: hub-smoke could not create ${message}`));
    assert.equal(result.stdout, '');
    assert.equal(existsSync(guard.SMOKE_COMMAND_MARKER), false);
    assert.deepEqual(readdirSync(scratch), []);
  });
}

test('init failure cleans the used error file and other temporary resources', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'hub-smoke-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'scratch');
  const guard = join(root, 'guard.sh');
  const marker = join(root, 'init-attempts');
  const fixtureScript = join(root, 'scripts', 'hub-smoke.sh');
  mkdirSync(scratch);
  mkdirSync(dirname(fixtureScript));
  mkdirSync(join(root, 'target', 'debug'), { recursive: true });
  copyFileSync(script, fixtureScript);
  // The script checks this path even when locus is supplied by a function.
  writeFileSync(join(root, 'target', 'debug', 'locus'), '#!/usr/bin/env bash\nexit 1\n', { mode: 0o755 });
  writeFileSync(join(scratch, 'unrelated'), 'keep');
  // Intercept even a locally built binary, and fail only after the error file
  // has been created and opened for the init command's stderr.
  writeFileSync(guard, `jq() { return 0; }
sleep() { :; }
locus() {
  if [[ "$1" == init ]]; then
    [[ -f "$SMOKE_ERR" && "$SMOKE_ERR" == "$TMPDIR"/locus-hub-smoke-err.* ]] || return 2
    printf 'attempt\\n' >> "$INIT_MARKER"
    printf 'synthetic init failure\\n' >&2
    return 1
  fi
  return 0
}
`);
  const result = spawnSync('bash', [fixtureScript], {
    env: { ...process.env, BASH_ENV: guard, TMPDIR: scratch, INIT_MARKER: marker },
    encoding: 'utf8',
    timeout: 15_000,
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /error: hub-smoke init\/pin failed after 3 attempts/);
  assert.match(result.stderr, /synthetic init failure/);
  assert.equal(readFileSync(marker, 'utf8'), 'attempt\nattempt\nattempt\n');
  assert.deepEqual(readdirSync(scratch), ['unrelated']);
  assert.equal(readFileSync(join(scratch, 'unrelated'), 'utf8'), 'keep');
});
