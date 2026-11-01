// Run: node --test scripts/dogfood.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'dogfood.sh');

test('temporary directory reference covers every dogfood script template', () => {
  const source = readFileSync(script, 'utf8');
  const reference = readFileSync(join(dirname(script), '..', 'docs', 'dogfood-temp-prefix-reference.md'), 'utf8');
  const templates = [...source.matchAll(/mktemp -d "\$\{TMPDIR:-\/tmp\}\/([^"/]+)"/g)]
    .map((match) => match[1]).sort();
  const documented = [...reference.matchAll(/\| `(locus-[\w.-]+\.XXXXXX)` \|/g)]
    .map((match) => match[1]).sort();
  assert.deepEqual(templates, [
    'locus-dogfood.XXXXXX',
    'locus-mt-dogfood.XXXXXX',
    'locus-sandbox-dogfood.XXXXXX',
  ]);
  assert.deepEqual(documented, templates);
});

test('probe role guide tracks each allocation and its readiness contract', () => {
  const source = readFileSync(script, 'utf8');
  const guide = readFileSync(join(dirname(script), '..', 'docs', 'dogfood-probe-roles.md'), 'utf8');
  const templates = [...source.matchAll(/mktemp -d "\$\{TMPDIR:-\/tmp\}\/([^"/]+)"/g)]
    .map((match) => match[1]).sort();
  const rows = [...guide.matchAll(/^\| `(locus-[\w.-]+\.XXXXXX)` \| [^|\n]+ \| [^|\n]+ \|$/gm)]
    .map((match) => match[1]).sort();
  assert.deepEqual(rows, templates);
  for (const [template, facts] of [
    ['locus-dogfood.XXXXXX', ['quickstart', 'agent setup', 'doctor', 'forensics', 'session', 'readiness']],
    ['locus-sandbox-dogfood.XXXXXX', ['fixture', 'sandbox-exec', 'unsupported-backend', 'blocks readiness']],
    ['locus-mt-dogfood.XXXXXX', ['DOGFOOD_MT=1', 'mints', '401', '200', 'revokes']],
  ]) {
    const row = guide.split('\n').find((line) => line.startsWith(`| \`${template}\` |`));
    for (const fact of facts) assert.ok(row.includes(fact), `${template}: missing ${fact}`);
  }
  for (const fact of ['DOGFOOD_USE_REAL_HOME=1', 'DOGFOOD_HOME', 'DOGFOOD_PACK',
    'DOGFOOD_SKIP_HUB_SMOKE=1', 'hub-smoke.sh', 'uncatchable']) {
    assert.ok(guide.includes(fact), `guide missing ${fact}`);
  }
});

test('probe cleanup guide covers both traps and the grant lifecycle', () => {
  const source = readFileSync(script, 'utf8');
  const guide = readFileSync(join(dirname(script), '..', 'docs', 'dogfood-probe-cleanup.md'), 'utf8');
  const sandbox = source.slice(source.indexOf('sandbox_attestation() {'), source.indexOf('mt_grant_probe() {'));
  const multiTenant = source.slice(source.indexOf('mt_grant_probe() {'), source.indexOf('# Allow the probe functions'));
  assert.match(sandbox, /sandbox_cleanup_root="\$probe_root"[\s\S]*trap 'rm -rf "\$sandbox_cleanup_root"' EXIT/);
  assert.match(multiTenant, /kill "\$mt_pid"[\s\S]*wait "\$mt_pid"[\s\S]*cd \/[\s\S]*rm -rf "\$probe_root"[\s\S]*trap 'mt_cleanup' EXIT/);
  for (const fact of ['locus-sandbox-dogfood.XXXXXX', 'locus-mt-dogfood.XXXXXX',
    'before entering its subshell', 'inside its subshell', 'not** in its exit trap',
    'uncatchable termination', 'failed removal']) {
    assert.ok(guide.includes(fact), `cleanup guide missing ${fact}`);
  }
});

test('sandbox probe removes its root after a missing binding failure', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-sandbox-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'temporary directory with spaces and $metacharacters');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'preserve');
  const probe = join(root, 'probe');
  const harness = String.raw`
source "$1"
locus() {
  if [[ "$1" == init ]]; then
    printf '%s\n' "$PWD" > "$PROBE_FILE"
    mkdir -p "$LOCUS_HOME"
    touch "$LOCUS_HOME/sentinel"
  fi
}
sandbox_attestation
`;
  const result = spawnSync('bash', ['-c', harness, 'dogfood-test', script], {
    cwd: root,
    env: { ...process.env, TMPDIR: scratch, PROBE_FILE: probe },
    encoding: 'utf8',
  });
  assert.equal(result.signal, null);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /sandbox probe binding missing/);
  const probeRoot = dirname(readFileSync(probe, 'utf8').trim());
  assert.match(probeRoot, /\/locus-sandbox-dogfood\.[^/]+$/);
  assert.equal(dirname(probeRoot), scratch);
  assert.equal(existsSync(probeRoot), false);
  assert.deepEqual(readdirSync(scratch), ['keep']);
  assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'preserve');
});

test('sandbox probe stops when its temporary directory cannot be allocated', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-sandbox-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const probe = join(root, 'probe');
  const harness = String.raw`
source "$1"
locus() { touch "$PROBE_FILE"; }
# Match the production caller, which disables errexit for this probe.
set +e
sandbox_attestation
exit "$?"
`;
  const result = spawnSync('bash', ['-c', harness, 'dogfood-test', script], {
    cwd: root,
    env: { ...process.env, TMPDIR: join(root, 'missing'), PROBE_FILE: probe },
    encoding: 'utf8',
  });
  assert.equal(result.signal, null);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /mktemp/);
  assert.equal(existsSync(probe), false);
});

test('isolated dogfood home is created under a custom TMPDIR and removed on exit', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-dogfood-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'temporary directory with spaces and $metacharacters');
  const fixtureRoot = join(root, 'fixture');
  const bin = join(fixtureRoot, 'target', 'debug');
  mkdirSync(scratch);
  mkdirSync(bin, { recursive: true });
  mkdirSync(join(fixtureRoot, 'scripts'));
  writeFileSync(join(scratch, 'keep'), 'preserve');
  writeFileSync(join(fixtureRoot, 'scripts', 'dogfood.sh'), readFileSync(script));
  writeFileSync(join(bin, 'cargo'), '#!/bin/sh\nexit 0\n', { mode: 0o755 });
  writeFileSync(join(bin, 'locus-mcp'), '#!/bin/sh\nexit 0\n', { mode: 0o755 });
  writeFileSync(join(bin, 'locus'), `#!/bin/bash
case "$1" in
  init)
    [[ -d "$LOCUS_HOME" && "\${LOCUS_HOME%/*}" == "$TMPDIR" ]] || exit 89
    printf '%s\\n' "$LOCUS_HOME" > "$PROBE_FILE"
    [[ "$PWD" == "$LOCUS_HOME/project" ]] || exit 90
    printf 'usable' > "$LOCUS_HOME/sentinel"
    ;;
  binding)
    [[ "$(cat "$LOCUS_HOME/sentinel")" == usable ]] || exit 91
    ;;
  --version) printf 'fixture\\n' ;;
  quickstart) exit 42 ;;
  *) exit 92 ;;
esac
`, { mode: 0o755 });
  const probe = join(root, 'probe');
  const result = spawnSync('bash', [join(fixtureRoot, 'scripts', 'dogfood.sh')], {
    cwd: root,
    env: {
      ...process.env,
      TMPDIR: scratch,
      PROBE_FILE: probe,
      DOGFOOD_HOME: '',
      DOGFOOD_USE_REAL_HOME: '0',
      DOGFOOD_SELF_TEST: '0',
    },
    encoding: 'utf8',
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 42, 'dogfood should reach the controlled quickstart failure');
  const home = readFileSync(probe, 'utf8').trim();
  assert.match(home, /\/locus-dogfood\.[^/]+$/);
  assert.equal(dirname(home), scratch);
  assert.equal(existsSync(home), false);
  assert.deepEqual(readdirSync(scratch), ['keep']);
  assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'preserve');
});

test('initialization failure removes only the allocated dogfood home', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-dogfood-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'temporary directory with spaces and $metacharacters');
  const existingHome = join(root, 'existing home');
  const fixtureRoot = join(root, 'fixture');
  const bin = join(fixtureRoot, 'target', 'debug');
  mkdirSync(scratch);
  mkdirSync(existingHome);
  mkdirSync(bin, { recursive: true });
  mkdirSync(join(fixtureRoot, 'scripts'));
  writeFileSync(join(existingHome, 'keep'), 'preserve');
  writeFileSync(join(scratch, 'keep'), 'preserve');
  writeFileSync(join(fixtureRoot, 'scripts', 'dogfood.sh'), readFileSync(script));
  writeFileSync(join(bin, 'cargo'), '#!/bin/sh\nexit 0\n', { mode: 0o755 });
  writeFileSync(join(bin, 'locus-mcp'), '#!/bin/sh\nexit 0\n', { mode: 0o755 });
  writeFileSync(join(bin, 'locus'), `#!/bin/bash
[[ "$1" == init && -d "$LOCUS_HOME" && "\${LOCUS_HOME%/*}" == "$TMPDIR" ]] || exit 89
[[ "$PWD" == "$LOCUS_HOME/project" ]] || exit 90
printf '%s\\n' "$LOCUS_HOME" > "$PROBE_FILE"
touch "$LOCUS_HOME/sentinel"
exit 42
`, { mode: 0o755 });
  const probe = join(root, 'probe');
  const result = spawnSync('bash', [join(fixtureRoot, 'scripts', 'dogfood.sh')], {
    cwd: root,
    env: {
      PATH: process.env.PATH,
      HOME: process.env.HOME,
      TMPDIR: scratch,
      PROBE_FILE: probe,
      DOGFOOD_HOME: existingHome,
      DOGFOOD_USE_REAL_HOME: '0',
    },
    encoding: 'utf8',
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 42, 'dogfood should reach the controlled initialization failure');
  const home = readFileSync(probe, 'utf8').trim();
  assert.match(home, /\/locus-dogfood\.[^/]+$/);
  assert.equal(dirname(home), scratch);
  assert.equal(existsSync(home), false);
  assert.deepEqual(readdirSync(scratch), ['keep']);
  assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'preserve');
  assert.equal(readFileSync(join(existingHome, 'keep'), 'utf8'), 'preserve');
});

test('a nonexistent TMPDIR fails before initializing the dogfood home', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-dogfood-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const existingHome = join(root, 'existing home');
  mkdirSync(existingHome);
  writeFileSync(join(existingHome, 'keep'), 'preserve');
  const fixtureRoot = join(root, 'fixture');
  const bin = join(fixtureRoot, 'target', 'debug');
  mkdirSync(bin, { recursive: true });
  mkdirSync(join(fixtureRoot, 'scripts'));
  writeFileSync(join(fixtureRoot, 'scripts', 'dogfood.sh'), readFileSync(script));
  writeFileSync(join(bin, 'cargo'), '#!/bin/sh\nexit 0\n', { mode: 0o755 });
  writeFileSync(join(bin, 'locus-mcp'), '#!/bin/sh\nexit 0\n', { mode: 0o755 });
  writeFileSync(join(bin, 'locus'), '#!/bin/sh\ntouch "$PROBE_FILE"\nexit 0\n', { mode: 0o755 });
  const probe = join(root, 'probe');
  const scratch = join(root, 'missing directory');
  const result = spawnSync('bash', [join(fixtureRoot, 'scripts', 'dogfood.sh')], {
    cwd: root,
    env: {
      ...process.env,
      TMPDIR: scratch,
      PROBE_FILE: probe,
      DOGFOOD_HOME: existingHome,
      DOGFOOD_USE_REAL_HOME: '0',
      DOGFOOD_SELF_TEST: '0',
    },
    encoding: 'utf8',
  });

  assert.equal(result.signal, null);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /mktemp/);
  assert.equal(existsSync(probe), false, 'Locus must not run after allocation fails');
  assert.equal(existsSync(scratch), false);
  assert.equal(readFileSync(join(existingHome, 'keep'), 'utf8'), 'preserve');
});

const harness = String.raw`
source "$1"
locus() {
  local stage="$1"
  [[ "$stage" != mcp ]] || stage="$2"
  printf '%s\n' "$stage" >> "$TRACE_FILE"
  if [[ "$stage" == init ]]; then
    printf '%s\n' "$PWD" > "$PROBE_FILE"
    mkdir -p "$LOCUS_HOME"
    touch "$LOCUS_HOME/sentinel"
  fi
  [[ "$stage" != "$FAIL_STAGE" ]] || return 1
  case "$stage" in
    mint) printf '%s\n' '{"grant_id":"grant_fixture","token":"lmt_fixture"}' ;;
    list)
      if [[ "$FAIL_STAGE" == http || "$FAIL_STAGE" == interrupt ]]; then
        printf '%s\n' '[{"grant_id":"grant_fixture","revoked":false,"expired":false}]'
      else
        printf '%s\n' '[]'
      fi ;;
  esac
}
curl() {
  if [[ "$*" == */health ]]; then
    for _ in {1..100}; do
      if [[ -f "$PID_FILE" ]]; then
        if [[ "$FAIL_STAGE" == interrupt ]]; then
          [[ -f "$LOCUS_HOME/sentinel" ]] && kill -0 "$(cat "$PID_FILE")" || return 1
          touch "$INTERRUPT_FILE"
          # The child targets its parent (the probe subshell), including on Bash 3.2.
          sh -c 'kill -TERM "$PPID"'
          return 1
        fi
        return 0
      fi
      /bin/sleep 0.01
    done
    return 1
  fi
  printf '500'
}
# Match the production caller: errexit is disabled within the function.
mt_grant_probe || exit 42
exit 0
`;

for (const [failure, expectedStages] of [
  ['init', ['init']],
  ['binding', ['init', 'binding']],
  ['mint', ['init', 'binding', 'mint']],
  ['roster', ['init', 'binding', 'mint', 'list']],
  ['http', ['init', 'binding', 'mint', 'list']],
]) {
  test(`multi-tenant probe cleans its temporary directory after ${failure} failure`, (t) => {
    const root = mkdtempSync(join(tmpdir(), 'locus-dogfood-test-'));
    const pidFile = join(root, 'server-pid');
    let serverStopped = false;
    t.after(() => {
      if (!serverStopped && existsSync(pidFile)) {
        try {
          process.kill(Number(readFileSync(pidFile, 'utf8').trim()));
        } catch (error) {
          if (error.code !== 'ESRCH') throw error;
        }
      }
      rmSync(root, { recursive: true, force: true });
    });
    const scratch = join(root, 'scratch');
    mkdirSync(scratch);
    writeFileSync(join(scratch, 'keep'), 'preserve');
    const trace = join(root, 'trace');
    const probe = join(root, 'probe');
    const server = join(root, 'server');
    writeFileSync(server, '#!/usr/bin/env bash\nprintf "%s\\n" "$$" > "$PID_FILE"\nexec sleep 120\n', { mode: 0o755 });
    const result = spawnSync('bash', ['-c', harness, 'dogfood-test', script], {
      cwd: root,
      env: { ...process.env, TMPDIR: scratch, TRACE_FILE: trace, PROBE_FILE: probe, FAIL_STAGE: failure, MCP_BIN: server, PID_FILE: pidFile },
      encoding: 'utf8',
    });

    assert.equal(result.signal, null);
    assert.equal(result.status, 42, result.stderr);
    assert.equal(result.stdout, '');
    assert.deepEqual(readFileSync(trace, 'utf8').trim().split('\n'), expectedStages);
    const probeRoot = readFileSync(probe, 'utf8').trim();
    assert.match(probeRoot, /\/locus-mt-dogfood\.[^/]+$/);
    assert.equal(dirname(probeRoot), scratch);
    assert.equal(existsSync(probeRoot), false);
    assert.deepEqual(readdirSync(scratch), ['keep']);
    assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'preserve');
    if (failure === 'http') {
      const pid = Number(readFileSync(pidFile, 'utf8').trim());
      assert.ok(Number.isInteger(pid) && pid > 0);
      assert.throws(() => process.kill(pid, 0), { code: 'ESRCH' });
      serverStopped = true;
    }
  });
}

test('multi-tenant probe cleans up after interruption with a running server', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-mt-interrupt-test-'));
  const pidFile = join(root, 'server-pid');
  let serverStopped = false;
  t.after(() => {
    if (!serverStopped && existsSync(pidFile)) {
      try {
        process.kill(Number(readFileSync(pidFile, 'utf8').trim()));
      } catch (error) {
        if (error.code !== 'ESRCH') throw error;
      }
    }
    rmSync(root, { recursive: true, force: true });
  });
  const scratch = join(root, 'temporary directory with spaces and $metacharacters');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'preserve');
  const trace = join(root, 'trace');
  const probe = join(root, 'probe');
  const interrupted = join(root, 'interrupted');
  const server = join(root, 'server');
  writeFileSync(server, '#!/usr/bin/env bash\nprintf "%s\\n" "$$" > "$PID_FILE"\nexec sleep 120\n', { mode: 0o755 });
  const result = spawnSync('bash', ['-c', harness, 'dogfood-test', script], {
    cwd: root,
    env: { ...process.env, TMPDIR: scratch, TRACE_FILE: trace, PROBE_FILE: probe, FAIL_STAGE: 'interrupt', MCP_BIN: server, PID_FILE: pidFile, INTERRUPT_FILE: interrupted },
    encoding: 'utf8',
    timeout: 30_000,
  });

  assert.ifError(result.error);
  assert.equal(result.signal, null);
  assert.equal(result.status, 42);
  assert.equal(existsSync(interrupted), true, 'signal must be sent after the server and probe home exist');
  assert.deepEqual(readFileSync(trace, 'utf8').trim().split('\n'), ['init', 'binding', 'mint', 'list']);
  const probeRoot = readFileSync(probe, 'utf8').trim();
  assert.match(probeRoot, /\/locus-mt-dogfood\.[^/]+$/);
  assert.equal(dirname(probeRoot), scratch);
  assert.equal(existsSync(probeRoot), false);
  assert.deepEqual(readdirSync(scratch), ['keep']);
  assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'preserve');
  const pid = Number(readFileSync(pidFile, 'utf8').trim());
  assert.ok(Number.isInteger(pid) && pid > 0);
  assert.throws(() => process.kill(pid, 0), { code: 'ESRCH' });
  serverStopped = true;
});


test('multi-tenant allocation failure stops before probe setup', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-mt-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const trace = join(root, 'trace');
  const missing = join(root, 'missing');
  const result = spawnSync('bash', ['-c', String.raw`
source "$1"
locus() { touch "$TRACE_FILE"; }
mt_grant_probe || { [[ "$?" == 1 ]] || exit 43; exit 42; }
exit 0
`, 'dogfood-test', script], {
    cwd: root,
    env: { ...process.env, TMPDIR: missing, TRACE_FILE: trace },
    encoding: 'utf8',
  });
  assert.equal(result.signal, null);
  assert.equal(result.status, 42);
  assert.match(result.stderr, /mktemp/);
  assert.equal(existsSync(trace), false);
  assert.equal(existsSync(missing), false);
});

test('temporary failure guide covers all allocation templates', () => {
  const source = readFileSync(script, 'utf8');
  const guide = readFileSync(join(dirname(script), '..', 'docs', 'dogfood-temp-failures.md'), 'utf8');
  const templates = [...source.matchAll(/mktemp -d "\$\{TMPDIR:-\/tmp\}\/([^"/]+)"/g)]
    .map((match) => match[1]).sort();
  const rows = [...guide.matchAll(/^\| `(locus-[\w.-]+\.XXXXXX)` \|/gm)]
    .map((match) => match[1]).sort();
  assert.deepEqual(rows, templates);
});
