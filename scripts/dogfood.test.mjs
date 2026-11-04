// Run: node --test scripts/dogfood.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, dirname, join, resolve } from 'node:path';
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

// The mktemp shim records the template sandbox_attestation passes and redirects
// anything outside $CONTROLLED into it, so the fallback case never touches /tmp.
function sandboxRootSelection(t, tmpdirEnv) {
  const root = mkdtempSync(join(tmpdir(), 'locus-sandbox-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const controlled = join(root, 'controlled');
  mkdirSync(controlled);
  const template = join(root, 'template');
  const created = join(root, 'created');
  // Plain strings, not String.raw: the shim needs bash's ${...} expansions.
  const harness = [
    'source "$1"',
    'mktemp() {',
    '  local last="${!#}" args=("$@") path',
    '  printf \'%s\\n\' "$last" > "$TEMPLATE_FILE"',
    '  [[ "$last" == "$CONTROLLED"/* ]] || args[$(($# - 1))]="$CONTROLLED/${last##*/}"',
    '  path="$(command mktemp "${args[@]}")" || return',
    '  printf \'%s\\n\' "$path" > "$CREATED_FILE"',
    '  printf \'%s\\n\' "$path"',
    '}',
    'locus() { return 0; }',
    'sandbox_attestation',
  ].join('\n');
  const env = { ...process.env, CONTROLLED: controlled, TEMPLATE_FILE: template, CREATED_FILE: created };
  delete env.TMPDIR;
  Object.assign(env, tmpdirEnv(controlled));
  const result = spawnSync('bash', ['-c', harness, 'dogfood-test', script], { cwd: root, env, encoding: 'utf8' });
  assert.equal(result.signal, null);
  assert.equal(result.status, 1, 'stubbed probe stops at the missing binding');
  return {
    controlled,
    template: readFileSync(template, 'utf8').trim(),
    created: readFileSync(created, 'utf8').trim(),
    left: readdirSync(controlled),
  };
}

test('sandbox probe root is created under a dedicated TMPDIR and removed', (t) => {
  const { controlled, template, created, left } = sandboxRootSelection(t, (dir) => ({ TMPDIR: dir }));
  assert.equal(template, `${controlled}/locus-sandbox-dogfood.XXXXXX`);
  assert.equal(dirname(created), controlled);
  assert.match(basename(created), /^locus-sandbox-dogfood\.[A-Za-z0-9]{6}$/);
  assert.deepEqual(left, []);
});

test('sandbox probe root stays directly beneath an explicit TMPDIR written with a trailing slash', (t) => {
  const { controlled, template, created, left } = sandboxRootSelection(t, (dir) => ({ TMPDIR: `${dir}/` }));
  assert.equal(template, `${controlled}//locus-sandbox-dogfood.XXXXXX`);
  assert.equal(resolve(dirname(created)), controlled);
  assert.match(basename(created), /^locus-sandbox-dogfood\.[A-Za-z0-9]{6}$/);
  assert.deepEqual(left, []);
});

for (const [label, tmpdirEnv] of [['unset', () => ({})], ['empty', () => ({ TMPDIR: '' })]]) {
  test(`sandbox probe root falls back to /tmp when TMPDIR is ${label}`, (t) => {
    const { template, left } = sandboxRootSelection(t, tmpdirEnv);
    assert.equal(template, '/tmp/locus-sandbox-dogfood.XXXXXX');
    assert.deepEqual(left, []);
  });
}

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

test('dogfood cleanup treats spaces and shell glob characters in TMPDIR literally', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-dogfood-literal-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, "temporary * [a] 'quoted' directory");
  const neighbor = join(root, "temporary match a 'quoted' directory");
  const bin = join(root, 'fixture', 'target', 'debug');
  const fixtureScript = join(root, 'fixture', 'scripts', 'dogfood.sh');
  mkdirSync(scratch);
  mkdirSync(neighbor);
  mkdirSync(bin, { recursive: true });
  mkdirSync(dirname(fixtureScript));
  writeFileSync(join(scratch, 'keep'), 'scratch');
  writeFileSync(join(neighbor, 'keep'), 'neighbor');
  writeFileSync(fixtureScript, readFileSync(script));
  writeFileSync(join(bin, 'cargo'), '#!/bin/sh\nexit 0\n', { mode: 0o755 });
  writeFileSync(join(bin, 'locus-mcp'), '#!/bin/sh\nexit 0\n', { mode: 0o755 });
  writeFileSync(join(bin, 'locus'), `#!/bin/bash
case "$1" in
  init)
    [[ -d "$LOCUS_HOME" && "\${LOCUS_HOME%/*}" == "$TMPDIR" ]] || exit 89
    printf '%s\\n' "$LOCUS_HOME" > "$PROBE_FILE"
    touch "$LOCUS_HOME/sentinel" ;;
  binding) [[ -f "$LOCUS_HOME/sentinel" ]] || exit 90 ;;
  --version) ;;
  quickstart) exit 42 ;;
  *) exit 91 ;;
esac
`, { mode: 0o755 });
  const probe = join(root, 'probe');
  const result = spawnSync('bash', [fixtureScript], {
    cwd: root,
    env: { PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: scratch, PROBE_FILE: probe },
    encoding: 'utf8',
  });

  assert.ifError(result.error);
  assert.equal(result.signal, null);
  assert.equal(result.status, 42, 'dogfood should reach the controlled quickstart failure');
  const home = readFileSync(probe, 'utf8').trim();
  assert.equal(dirname(home), scratch);
  assert.match(home, /\/locus-dogfood\.[^/]+$/);
  assert.equal(existsSync(home), false);
  assert.deepEqual(readdirSync(scratch), ['keep']);
  assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'scratch');
  assert.equal(readFileSync(join(neighbor, 'keep'), 'utf8'), 'neighbor');
});

// The mktemp stub records the template the script mainline passes and
// redirects anything outside $CONTROLLED into it, so the fallback cases
// never touch /tmp. It lives in the fixture's target/debug, which dogfood.sh
// prepends to PATH.
function dogfoodHomeRootSelection(t, tmpdirEnv) {
  const root = mkdtempSync(join(tmpdir(), 'locus-dogfood-select-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const controlled = join(root, 'controlled');
  const fixtureRoot = join(root, 'fixture');
  const bin = join(fixtureRoot, 'target', 'debug');
  mkdirSync(controlled);
  mkdirSync(bin, { recursive: true });
  mkdirSync(join(fixtureRoot, 'scripts'));
  writeFileSync(join(fixtureRoot, 'scripts', 'dogfood.sh'), readFileSync(script));
  writeFileSync(join(bin, 'cargo'), '#!/bin/sh\nexit 0\n', { mode: 0o755 });
  writeFileSync(join(bin, 'locus-mcp'), '#!/bin/sh\nexit 0\n', { mode: 0o755 });
  writeFileSync(join(bin, 'mktemp'), `#!/usr/bin/env bash
last="\${!#}"
printf '%s\\n' "$last" >> "$MKTEMP_LOG"
args=("$@")
[[ "$last" == "$CONTROLLED"/* ]] || args[$(($# - 1))]="$CONTROLLED/\${last##*/}"
path="$(PATH=/usr/bin:/bin command mktemp "\${args[@]}")" || exit
printf '%s\\n' "$path" >> "$CREATED_LOG"
printf '%s\\n' "$path"
`, { mode: 0o755 });
  writeFileSync(join(bin, 'locus'), `#!/bin/bash
case "$1" in
  init) printf '%s\\n' "$LOCUS_HOME" > "$PROBE_FILE" ;;
  quickstart) exit 42 ;;
esac
`, { mode: 0o755 });
  const templates = join(root, 'templates');
  const created = join(root, 'created');
  const probe = join(root, 'probe');
  const env = { PATH: process.env.PATH, HOME: process.env.HOME, CONTROLLED: controlled,
    MKTEMP_LOG: templates, CREATED_LOG: created, PROBE_FILE: probe };
  Object.assign(env, tmpdirEnv(controlled));
  const result = spawnSync('bash', [join(fixtureRoot, 'scripts', 'dogfood.sh')], {
    cwd: root, env, encoding: 'utf8',
  });
  assert.equal(result.signal, null);
  assert.equal(result.status, 42, 'dogfood should reach the controlled quickstart failure');
  return {
    controlled,
    templates: readFileSync(templates, 'utf8').trim().split('\n'),
    created: readFileSync(created, 'utf8').trim(),
    home: readFileSync(probe, 'utf8').trim(),
    left: readdirSync(controlled),
  };
}

test('dogfood home root is selected and created under an explicit TMPDIR', (t) => {
  const { controlled, templates, created, home, left } = dogfoodHomeRootSelection(t, (dir) => ({ TMPDIR: dir }));
  assert.deepEqual(templates, [`${controlled}/locus-dogfood.XXXXXX`]);
  assert.equal(dirname(created), controlled);
  assert.match(basename(created), /^locus-dogfood\.[A-Za-z0-9]{6}$/);
  assert.equal(home, created);
  assert.equal(existsSync(created), false);
  assert.deepEqual(left, []);
});

test('dogfood home root stays directly beneath an explicit TMPDIR written with a trailing slash', (t) => {
  const { controlled, templates, created, home, left } = dogfoodHomeRootSelection(t, (dir) => ({ TMPDIR: `${dir}/` }));
  assert.deepEqual(templates, [`${controlled}//locus-dogfood.XXXXXX`]);
  assert.equal(resolve(dirname(created)), controlled);
  assert.match(basename(created), /^locus-dogfood\.[A-Za-z0-9]{6}$/);
  assert.equal(home, created);
  assert.equal(existsSync(created), false);
  assert.deepEqual(left, []);
});

for (const [label, tmpdirEnv] of [['unset', () => ({})], ['empty', () => ({ TMPDIR: '' })]]) {
  test(`dogfood home root falls back to /tmp when TMPDIR is ${label}`, (t) => {
    const { controlled, templates, created, home, left } = dogfoodHomeRootSelection(t, tmpdirEnv);
    assert.deepEqual(templates, ['/tmp/locus-dogfood.XXXXXX']);
    assert.equal(dirname(created), controlled);
    assert.match(basename(created), /^locus-dogfood\.[A-Za-z0-9]{6}$/);
    assert.equal(home, created);
    assert.equal(existsSync(created), false);
    assert.deepEqual(left, []);
  });
}

function writeSuccessFixture(root) {
  const fixtureRoot = join(root, 'fixture');
  const bin = join(fixtureRoot, 'target', 'debug');
  mkdirSync(bin, { recursive: true });
  mkdirSync(join(fixtureRoot, 'scripts'));
  writeFileSync(join(fixtureRoot, 'scripts', 'dogfood.sh'), readFileSync(script));
  writeFileSync(join(fixtureRoot, 'scripts', 'dogfood-ready.jq'), readFileSync(join(dirname(script), 'dogfood-ready.jq')));
  writeFileSync(join(fixtureRoot, 'scripts', 'hub-smoke.sh'), '#!/bin/sh\nexit 0\n');
  writeFileSync(join(bin, 'cargo'), '#!/bin/sh\nexit 0\n', { mode: 0o755 });
  writeFileSync(join(bin, 'uname'), '#!/bin/sh\nprintf Darwin\n', { mode: 0o755 });
  writeFileSync(join(bin, 'locus-mcp'), '#!/bin/sh\nexit 0\n', { mode: 0o755 });

  const pin = { alias: 'dogfood', tenant: 'dogfood', binding_id: 'bnd_fixture', expires_at: '2030-01-01', seal_ok: true, expired: false };
  const providers = [{ provider: 'github' }];
  const runtime = { pinned: true, seal_ok: true, binding_present: true, binding_id_match: true,
    tenant_match: true, providers_match: true, frozen: false, expired: false, ok: true, issues: [],
    session_id: 'ses_fixture', binding_alias: pin.alias, binding_id_session: pin.binding_id,
    binding_id_file: pin.binding_id, tenant_session: pin.tenant, tenant_file: pin.tenant, providers };
  const doctor = { home: '/fixture', pin, pin_seal_ok: true, verdict: 'SAFE', ok: true, unresolved_phm: [], runtime };
  const report = { status: 'ready', ready: true, exit_code: 0, pin, status_oneline: 'dogfood:dogfood',
    env_session_id: null, home: doctor.home, doctor, required_servers: ['locus', 'phantom'], mcp_command: 'locus-mcp' };
  const verify = { kind: 'session', session_ok: true, doctor,
    whoami: { session_id: runtime.session_id, binding_alias: pin.alias, binding_id: pin.binding_id,
      tenant: pin.tenant, expires_at: pin.expires_at, worker_home: '/fixture/worker', providers, seal_ok: true, frozen: false },
    safe_next: { ready: true, action: 'ready', binding: pin.alias, tenant: pin.tenant } };
  writeFileSync(join(root, 'report.json'), JSON.stringify(report));
  writeFileSync(join(root, 'verify.json'), JSON.stringify(verify));
  writeFileSync(join(bin, 'locus'), `#!/bin/bash
case "$1 $2" in
  'init ')
    if [[ "$LOCUS_HOME" == "$TMPDIR"/locus-dogfood.* ]]; then
      [[ "$PWD" == "$LOCUS_HOME/project" && -d "$LOCUS_HOME" ]] || exit 89
      printf '%s\\n' "$LOCUS_HOME" > "$PROBE_FILE"
      touch "$LOCUS_HOME/sentinel"
    fi ;;
  'binding add') mkdir -p "$LOCUS_HOME/bindings"; touch "$LOCUS_HOME/bindings/$3.toml" ;;
  'exec --no-resolve') printf '%s\\n' '{"id":2,"result":{"tools":[]}}' '{"id":3,"result":{"isError":false,"content":[{"text":"{\\"isError\\":false,\\"content\\":[{\\"text\\":\\"sandboxed=1 backend=sandbox-exec\\"}]}"}]}}' ;;
  'agent report') cat "$FIXTURE_ROOT/report.json" ;;
  'verify session') [[ "\${3:-}" == --help ]] || cat "$FIXTURE_ROOT/verify.json" ;;
  'forensics export') printf '{}\\n' > "$4" ;;
  'goal status') exit 1 ;;
  '--version '|'quickstart '|'pin sandbox-probe'|'agent setup'|'doctor ') ;;
  *) exit 90 ;;
esac
`, { mode: 0o755 });
  return fixtureRoot;
}

test('successful dogfood run removes its isolated home and preserves neighboring files', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-dogfood-success-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'temporary directory with spaces and $metacharacters');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'preserve');
  const fixtureRoot = writeSuccessFixture(root);

  const probe = join(root, 'probe');
  const result = spawnSync('bash', [join(fixtureRoot, 'scripts', 'dogfood.sh')], {
    cwd: root,
    env: { PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: scratch, PROBE_FILE: probe, FIXTURE_ROOT: root },
    encoding: 'utf8',
  });
  assert.equal(result.signal, null);
  assert.equal(result.status, 0, 'the fixture must reach a normal successful exit');
  assert.match(result.stdout, /DOGFOOD READY/);
  const home = readFileSync(probe, 'utf8').trim();
  assert.match(home, /\/locus-dogfood\.[^/]+$/);
  assert.equal(dirname(home), scratch);
  assert.equal(existsSync(home), false);
  assert.deepEqual(readdirSync(scratch), ['keep']);
  assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'preserve');
});

test('successful dogfood run ignores an inherited DOGFOOD_HOME and leaves it intact', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-dogfood-inherited-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'tmp');
  const inherited = join(root, 'caller-owned');
  mkdirSync(scratch);
  mkdirSync(inherited);
  writeFileSync(join(inherited, 'keep'), 'preserve');
  const fixtureRoot = writeSuccessFixture(root);

  const probe = join(root, 'probe');
  const result = spawnSync('bash', [join(fixtureRoot, 'scripts', 'dogfood.sh')], {
    cwd: root,
    env: { PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: scratch, PROBE_FILE: probe,
      FIXTURE_ROOT: root, DOGFOOD_HOME: inherited },
    encoding: 'utf8',
  });
  assert.equal(result.status, 0);
  assert.match(result.stdout, /DOGFOOD READY/);
  const home = readFileSync(probe, 'utf8').trim();
  assert.notEqual(home, inherited);
  assert.equal(dirname(home), scratch);
  assert.deepEqual(readdirSync(scratch), []);
  assert.equal(readFileSync(join(inherited, 'keep'), 'utf8'), 'preserve');
});

test('two dogfood invocations create distinct homes and each removes its own', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-dogfood-distinct-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'tmp');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'preserve');
  const fixtureRoot = writeSuccessFixture(root);

  const homes = [1, 2].map((n) => {
    const probe = join(root, `probe-${n}`);
    const result = spawnSync('bash', [join(fixtureRoot, 'scripts', 'dogfood.sh')], {
      cwd: root,
      env: { PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: scratch, PROBE_FILE: probe,
        FIXTURE_ROOT: root },
      encoding: 'utf8',
    });
    assert.equal(result.signal, null);
    assert.equal(result.status, 0, `run ${n} must succeed`);
    assert.match(result.stdout, /DOGFOOD READY/);
    return readFileSync(probe, 'utf8').trim();
  });

  assert.notEqual(homes[0], homes[1], 'each invocation must get its own DOGFOOD_HOME');
  for (const home of homes) {
    assert.match(home, /\/locus-dogfood\.[^/]+$/);
    assert.equal(dirname(home), scratch);
    assert.equal(existsSync(home), false, `removed on exit: ${home}`);
  }
  assert.deepEqual(readdirSync(scratch), ['keep']);
});

test('dogfood home root is created under a supplied TMPDIR with private permissions', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-dogfood-perms-'));
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
    [[ "$PWD" == "$LOCUS_HOME/project" ]] || exit 90
    printf '%s\\n' "$LOCUS_HOME" > "$PROBE_FILE"
    if [[ "$(uname -s)" == Darwin ]]; then
      stat -f %Lp "$LOCUS_HOME" > "$MODE_FILE"
    else
      stat -c %a "$LOCUS_HOME" > "$MODE_FILE"
    fi
    printf 'usable' > "$LOCUS_HOME/sentinel"
    ;;
  binding)
    [[ "$(cat "$LOCUS_HOME/sentinel")" == usable ]] || exit 91
    ;;
  --version) ;;
  quickstart) exit 42 ;;
  *) exit 92 ;;
esac
`, { mode: 0o755 });
  const probe = join(root, 'probe');
  const modeFile = join(root, 'mode');
  const result = spawnSync('bash', [join(fixtureRoot, 'scripts', 'dogfood.sh')], {
    cwd: root,
    env: { PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: scratch,
      PROBE_FILE: probe, MODE_FILE: modeFile },
    encoding: 'utf8',
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 42, 'dogfood should reach the controlled quickstart failure');
  const home = readFileSync(probe, 'utf8').trim();
  assert.match(home, /\/locus-dogfood\.[^/]+$/);
  assert.equal(dirname(home), scratch);
  assert.equal(readFileSync(modeFile, 'utf8').trim(), '700',
    'mktemp -d must allocate the dogfood home with owner-only permissions');
  assert.equal(existsSync(home), false);
  assert.deepEqual(readdirSync(scratch), ['keep']);
  assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'preserve');
});

test('real-home mode selects no temporary root and preserves the supplied DOGFOOD_HOME', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-dogfood-real-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'temporary directory with spaces and $metacharacters');
  const callerHome = join(root, 'caller home');
  mkdirSync(scratch);
  mkdirSync(callerHome);
  writeFileSync(join(scratch, 'keep'), 'preserve');
  writeFileSync(join(callerHome, 'keep'), 'preserve');
  const fixtureRoot = writeSuccessFixture(root);
  const locusBin = join(fixtureRoot, 'target', 'debug', 'locus');
  const stub = readFileSync(locusBin, 'utf8').replace(
    "'forensics export') printf '{}\\n' > \"$4\" ;;",
    "'forensics export') printf '%s\\n' \"$LOCUS_HOME\" > \"$HOME_RECORD\"; printf '{}\\n' > \"$4\" ;;");
  assert.notEqual(stub, readFileSync(locusBin, 'utf8'), 'fixture must be patched');
  writeFileSync(locusBin, stub, { mode: 0o755 });

  const probe = join(root, 'probe');
  const record = join(root, 'home-record');
  const result = spawnSync('bash', [join(fixtureRoot, 'scripts', 'dogfood.sh')], {
    cwd: root,
    env: { PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: scratch,
      PROBE_FILE: probe, FIXTURE_ROOT: root, HOME_RECORD: record,
      DOGFOOD_USE_REAL_HOME: '1', DOGFOOD_HOME: callerHome, DOGFOOD_PACK: join(root, 'pack.json') },
    encoding: 'utf8',
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /using real LOCUS_HOME/);
  assert.match(result.stdout, /DOGFOOD READY/);
  assert.equal(readFileSync(record, 'utf8').trim(), callerHome,
    'real mode must run against the supplied DOGFOOD_HOME, not an allocated root');
  assert.equal(existsSync(probe), false, 'isolated-home probe path never ran');
  assert.deepEqual(readdirSync(scratch), ['keep'],
    'no locus-dogfood.* root is allocated under TMPDIR in real-home mode');
  assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'preserve');
  assert.equal(readFileSync(join(callerHome, 'keep'), 'utf8'), 'preserve',
    'caller-owned home must survive the exit trap');
});

test('project setup failure removes the allocated dogfood home before initialization', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-dogfood-setup-failure-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'isolated temporary parent');
  const fixtureRoot = join(root, 'fixture');
  const bin = join(fixtureRoot, 'target', 'debug');
  mkdirSync(scratch);
  mkdirSync(bin, { recursive: true });
  mkdirSync(join(fixtureRoot, 'scripts'));
  writeFileSync(join(scratch, 'keep'), 'preserve');
  writeFileSync(join(fixtureRoot, 'scripts', 'dogfood.sh'), readFileSync(script));
  writeFileSync(join(bin, 'cargo'), '#!/bin/sh\nexit 0\n', { mode: 0o755 });
  writeFileSync(join(bin, 'locus-mcp'), '#!/bin/sh\nexit 0\n', { mode: 0o755 });
  writeFileSync(join(bin, 'locus'), '#!/bin/sh\ntouch "$LOCUS_CALLED"\nexit 0\n', { mode: 0o755 });
  writeFileSync(join(bin, 'mkdir'), `#!/bin/bash
[[ "$1" == -p && "$2" == "$TMPDIR"/locus-dogfood.*/project ]] || exit 89
home="${'$'}{2%/project}"
[[ -d "$home" ]] || exit 90
printf '%s\\n' "$home" > "$PROBE_FILE"
touch "$home/sentinel"
exit 42
`, { mode: 0o755 });
  const probe = join(root, 'probe');
  const locusCalled = join(root, 'locus-called');
  const result = spawnSync('bash', [join(fixtureRoot, 'scripts', 'dogfood.sh')], {
    cwd: root,
    env: { PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: scratch, PROBE_FILE: probe, LOCUS_CALLED: locusCalled },
    encoding: 'utf8',
  });

  assert.ifError(result.error);
  assert.equal(result.signal, null);
  assert.equal(result.status, 42, 'dogfood should propagate the injected project setup failure');
  const home = readFileSync(probe, 'utf8').trim();
  assert.match(home, /\/locus-dogfood\.[^/]+$/);
  assert.equal(dirname(home), scratch);
  assert.equal(existsSync(home), false);
  assert.equal(existsSync(locusCalled), false, 'Locus must not run after project setup fails');
  assert.doesNotMatch(result.stdout, /DOGFOOD READY/);
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

test('SIGTERM removes the isolated dogfood home under a custom TMPDIR', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-dogfood-signal-'));
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
[[ "$1" == init && -d "$LOCUS_HOME" && "\${LOCUS_HOME%/*}" == "$TMPDIR" ]] || exit 89
[[ "$PWD" == "$LOCUS_HOME/project" ]] || exit 90
printf '%s\\n' "$LOCUS_HOME" > "$PROBE_FILE"
touch "$LOCUS_HOME/sentinel"
kill -TERM "$PPID"
`, { mode: 0o755 });
  const probe = join(root, 'probe');
  const result = spawnSync('bash', [join(fixtureRoot, 'scripts', 'dogfood.sh')], {
    cwd: root,
    env: { PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: scratch, PROBE_FILE: probe },
    encoding: 'utf8',
    timeout: 30_000,
  });

  assert.ifError(result.error);
  assert.equal(result.signal, 'SIGTERM');
  const home = readFileSync(probe, 'utf8').trim();
  assert.match(home, /\/locus-dogfood\.[^/]+$/);
  assert.equal(dirname(home), scratch);
  assert.equal(existsSync(home), false);
  assert.deepEqual(readdirSync(scratch), ['keep']);
  assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'preserve');
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

test('successful multi-tenant probe removes its temporary directory and stops its server', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-mt-success-test-'));
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
  const server = join(root, 'server');
  writeFileSync(server, String.raw`#!/usr/bin/env node
const http = require('node:http');
const fs = require('node:fs');
if (process.argv[2] !== '--http' || process.argv[4] !== '--multi-tenant') process.exit(2);
const [host, port] = process.argv[3].split(':');
const server = http.createServer((req, res) => {
  if (req.method === 'GET' && req.url === '/health') {
    res.writeHead(200).end();
    return;
  }
  let body = '';
  req.on('data', (chunk) => { body += chunk; });
  req.on('end', () => {
    let init;
    try { init = JSON.parse(body); } catch { res.writeHead(400).end(); return; }
    if (req.method !== 'POST' || req.url !== '/mcp' ||
        req.headers.authorization !== 'Bearer ' + process.env.LOCUS_MCP_HTTP_TOKEN ||
        req.headers['content-type'] !== 'application/json' ||
        req.headers.accept !== 'application/json' || init.method !== 'initialize') {
      res.writeHead(400).end();
      return;
    }
    const token = req.headers['x-locus-tenant-token'];
    const revoked = fs.existsSync(process.env.STATE_DIR + '/revoked');
    if (token && token !== 'lmt_fixture') { res.writeHead(400).end(); return; }
    const stage = !token ? 'tenantless' : revoked ? 'revoked' : 'granted';
    fs.appendFileSync(process.env.TRACE_FILE, stage + '\n');
    res.writeHead(stage === 'granted' ? 200 : 401).end('invalid_grant');
  });
});
server.listen(Number(port), host, () => fs.writeFileSync(process.env.PID_FILE, String(process.pid)));
`, { mode: 0o755 });
  const successHarness = String.raw`
source "$1"
locus() {
  local stage="$1"
  [[ "$stage" != mcp ]] || stage="$2"
  printf '%s\n' "$stage" >> "$TRACE_FILE"
  case "$stage" in
    init)
      printf '%s\n' "$PWD" > "$PROBE_FILE"
      mkdir -p "$LOCUS_HOME"
      touch "$LOCUS_HOME/sentinel" ;;
    binding) [[ -f "$LOCUS_HOME/sentinel" ]] || return 1 ;;
    mint) printf '%s\n' '{"grant_id":"grant_fixture","token":"lmt_fixture"}' ;;
    list)
      [[ -f "$LOCUS_HOME/sentinel" && "$LOCUS_HOME" == "$PWD/home" ]] || return 1
      if [[ -f "$STATE_DIR/revoked" ]]; then
        printf '%s\n' '[]'
      else
        printf '%s\n' '[{"grant_id":"grant_fixture","revoked":false,"expired":false}]'
      fi ;;
    revoke) [[ "$3" == grant_fixture ]] || return 1; touch "$STATE_DIR/revoked" ;;
    *) return 1 ;;
  esac
}
caller_pwd="$PWD"
caller_home="$LOCUS_HOME"
caller_session="$LOCUS_SESSION_ID"
mt_grant_probe || exit 42
[[ ! -e "$(cat "$PROBE_FILE")" ]] || exit 43
[[ "$PWD" == "$caller_pwd" && "$LOCUS_HOME" == "$caller_home" && "$LOCUS_SESSION_ID" == "$caller_session" ]] || exit 44
printf 'probe returned after cleanup\n'
`;
  const result = spawnSync('bash', ['-c', successHarness, 'dogfood-test', script], {
    cwd: root,
    env: { ...process.env, TMPDIR: scratch, TRACE_FILE: trace, PROBE_FILE: probe, STATE_DIR: root, MCP_BIN: server, PID_FILE: pidFile,
      LOCUS_HOME: join(root, 'caller-home'), LOCUS_SESSION_ID: 'caller-session' },
    encoding: 'utf8',
    timeout: 30_000,
  });

  assert.ifError(result.error);
  assert.equal(result.signal, null);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout, 'probe returned after cleanup\n');
  assert.deepEqual(readFileSync(trace, 'utf8').trim().split('\n'),
    ['init', 'binding', 'mint', 'list', 'tenantless', 'granted', 'revoke', 'revoked', 'list']);
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

test('multi-tenant probe root is created under the supplied TMPDIR and removed on failure', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-mt-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'temporary directory with spaces and $metacharacters');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'preserve');
  const probe = join(root, 'probe');
  // Stub locus: record where the probe root was chosen, then fail init so the
  // probe stops before it starts any server.
  const harness = String.raw`
source "$1"
locus() {
  if [[ "$1" == init ]]; then
    printf '%s\n%s\n' "$LOCUS_HOME" "$PWD" > "$PROBE_FILE"
    mkdir -p "$LOCUS_HOME"
    touch "$LOCUS_HOME/sentinel"
    return 1
  fi
}
set +e
mt_grant_probe
exit "$?"
`;
  const result = spawnSync('bash', ['-c', harness, 'dogfood-test', script], {
    cwd: root,
    env: { ...process.env, TMPDIR: scratch, PROBE_FILE: probe },
    encoding: 'utf8',
  });
  assert.equal(result.signal, null);
  assert.equal(result.status, 1);
  const [home, cwd] = readFileSync(probe, 'utf8').trim().split('\n');
  const probeRoot = dirname(home);
  assert.match(probeRoot, /\/locus-mt-dogfood\.[^/]+$/);
  assert.equal(dirname(probeRoot), scratch);
  assert.equal(basename(cwd), basename(probeRoot));
  assert.equal(existsSync(probeRoot), false);
  assert.deepEqual(readdirSync(scratch), ['keep']);
});

test('two concurrent multi-tenant probes hold distinct live probe_root directories', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-mt-distinct-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'temporary directory with spaces and $metacharacters');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'preserve');
  const roots = join(root, 'roots');
  const bothAlive = join(root, 'both-alive');
  // Two probes run concurrently. Each init stub records its probe_root (the
  // cwd once the probe has cd'd into it) then parks until RELEASE appears, so
  // the parent can prove both roots exist at the same time before releasing
  // them. Each probe then fails at init and its EXIT trap removes only its
  // own root.
  const harness = String.raw`
source "$1"
locus() {
  [[ "$1" == init ]] || return 1
  printf '%s\n' "$PWD" > "$REC_FILE"
  local n=0
  while [[ ! -f "$RELEASE" && $n -lt 400 ]]; do
    sleep 0.05
    n=$((n + 1))
  done
  return 1
}
( REC_FILE="$R1"; mt_grant_probe ) & p1=$!
( REC_FILE="$R2"; mt_grant_probe ) & p2=$!
n=0
while [[ $n -lt 400 ]] && [[ ! -s "$R1" || ! -s "$R2" ]]; do
  sleep 0.05
  n=$((n + 1))
done
r1=""
r2=""
if [[ -s "$R1" ]]; then r1="$(cat "$R1")"; fi
if [[ -s "$R2" ]]; then r2="$(cat "$R2")"; fi
printf '%s\n%s\n' "$r1" "$r2" > "$ROOTS_FILE"
if [[ -n "$r1" && -n "$r2" && -d "$r1" && -d "$r2" && "$r1" != "$r2" ]]; then
  touch "$BOTH_ALIVE"
fi
touch "$RELEASE"
s1=0
s2=0
wait "$p1" || s1=$?
wait "$p2" || s2=$?
[[ -f "$BOTH_ALIVE" ]] || exit 43
[[ $s1 -eq 1 && $s2 -eq 1 ]] || exit 44
[[ ! -e "$r1" && ! -e "$r2" ]] || exit 45
`;
  const result = spawnSync('bash', ['-c', harness, 'dogfood-test', script], {
    cwd: root,
    env: { ...process.env, TMPDIR: scratch, R1: join(root, 'rec-1'), R2: join(root, 'rec-2'),
      RELEASE: join(root, 'release'), ROOTS_FILE: roots, BOTH_ALIVE: bothAlive },
    encoding: 'utf8',
    timeout: 30_000,
  });
  assert.ifError(result.error);
  assert.equal(result.signal, null);
  assert.equal(result.status, 0, `concurrent probe harness failed: ${result.stderr}`);
  assert.equal(existsSync(bothAlive), true,
    'both probe roots must exist at the same time and have different paths');
  const [first, second] = readFileSync(roots, 'utf8').trim().split('\n');
  assert.notEqual(first, second);
  for (const probeRoot of [first, second]) {
    assert.match(probeRoot, /\/locus-mt-dogfood\.[^/]+$/);
    assert.equal(dirname(probeRoot), scratch);
    assert.equal(existsSync(probeRoot), false, `removed on probe exit: ${probeRoot}`);
  }
  assert.deepEqual(readdirSync(scratch), ['keep']);
});

test('multi-tenant probe directory is created beneath the configured TMPDIR', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-mt-live-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'temporary directory with spaces and $metacharacters');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'preserve');
  const probe = join(root, 'probe');
  const release = join(root, 'release');
  const modeFile = join(root, 'mode');
  const homeFile = join(root, 'home');
  // The init stub records the probe root (its cwd once the probe has cd'd into
  // it) then parks until RELEASE appears, so the parent can observe the live
  // directory before releasing the probe. Returning 1 stops the probe before
  // any grant or server work; its EXIT trap removes only the probe root.
  const harness = String.raw`
source "$1"
locus() {
  if [[ "$1" == init ]]; then
    printf '%s\n' "$PWD" > "$PROBE_FILE"
    printf '%s\n' "$LOCUS_HOME" > "$HOME_FILE"
    local tries=0
    while [[ ! -f "$RELEASE" && $tries -lt 400 ]]; do
      sleep 0.05
      tries=$((tries + 1))
    done
    return 1
  fi
  return 1
}
( mt_grant_probe ) & pid=$!
tries=0
while [[ ! -s "$PROBE_FILE" && $tries -lt 400 ]]; do
  sleep 0.05
  tries=$((tries + 1))
done
probe_root=""
if [[ -s "$PROBE_FILE" ]]; then
  probe_root="$(cat "$PROBE_FILE")"
fi
if [[ -z "$probe_root" ]]; then
  printf 'probe root was never recorded\n' >&2
  touch "$RELEASE"
  exit 43
fi
if [[ ! -d "$probe_root" || "${'$'}{probe_root%/*}" != "$TMPDIR" || "$probe_root" != "$TMPDIR"/locus-mt-dogfood.* ]]; then
  printf 'probe root is not a live directory beneath TMPDIR: %s\n' "$probe_root" >&2
  touch "$RELEASE"
  exit 44
fi
if ! kill -0 "$pid" 2>/dev/null; then
  printf 'probe exited before its root was observed\n' >&2
  touch "$RELEASE"
  exit 45
fi
if [[ "$(uname -s)" == Darwin ]]; then
  stat -f %Lp "$probe_root" > "$MODE_FILE"
else
  stat -c %a "$probe_root" > "$MODE_FILE"
fi
touch "$RELEASE"
status=0
wait "$pid" || status=$?
[[ "$status" -eq 1 ]] || exit 46
[[ ! -e "$probe_root" ]] || exit 47
`;
  const result = spawnSync('bash', ['-c', harness, 'dogfood-test', script], {
    cwd: root,
    env: { ...process.env, TMPDIR: scratch, PROBE_FILE: probe, MODE_FILE: modeFile, HOME_FILE: homeFile, RELEASE: release },
    encoding: 'utf8',
    timeout: 30_000,
  });
  assert.ifError(result.error);
  assert.equal(result.signal, null);
  assert.equal(result.status, 0, result.stderr);
  const probeRoot = readFileSync(probe, 'utf8').trim();
  assert.match(basename(probeRoot), /^locus-mt-dogfood\.[^/]+$/);
  assert.equal(dirname(probeRoot), scratch);
  assert.equal(readFileSync(homeFile, 'utf8').trim(), join(probeRoot, 'home'),
    'the probe must use a home inside its isolated root');
  assert.equal(readFileSync(modeFile, 'utf8').trim(), '700',
    'mktemp -d must allocate the probe root with owner-only permissions');
  assert.equal(existsSync(probeRoot), false);
  assert.deepEqual(readdirSync(scratch), ['keep']);
  assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'preserve');
});

test('default pack path and project dir stay one argument under a space-containing TMPDIR', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-dogfood-pack-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'tmp with  spaces');
  mkdirSync(scratch);
  const fixtureRoot = writeSuccessFixture(root);
  const locusBin = join(fixtureRoot, 'target', 'debug', 'locus');
  const stub = readFileSync(locusBin, 'utf8').replace(
    "'forensics export') printf '{}\\n' > \"$4\" ;;",
    "'forensics export') printf '%s\\n' \"$#\" \"$3\" \"$4\" > \"$PACK_RECORD\"; printf '{}\\n' > \"$4\" ;;");
  assert.notEqual(stub, readFileSync(locusBin, 'utf8'), 'fixture must be patched');
  writeFileSync(locusBin, stub, { mode: 0o755 });

  const probe = join(root, 'probe');
  const record = join(root, 'pack-record');
  const result = spawnSync('bash', [join(fixtureRoot, 'scripts', 'dogfood.sh')], {
    cwd: root,
    env: { PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: scratch, PROBE_FILE: probe,
      FIXTURE_ROOT: root, PACK_RECORD: record },
    encoding: 'utf8',
  });
  assert.equal(result.status, 0, result.stderr);
  const home = readFileSync(probe, 'utf8').trim();
  assert.equal(dirname(home), scratch);
  assert.deepEqual(readFileSync(record, 'utf8').trim().split('\n'),
    ['4', '--out', join(home, 'pack.json')]);
  assert.ok(result.stdout.includes(`isolated LOCUS_HOME=${home}`));
  assert.deepEqual(readdirSync(scratch), []);
});

for (const setting of ['unset', 'empty']) {
  test(`multi-tenant probe root falls back to /tmp when TMPDIR is ${setting}`, (t) => {
    const root = mkdtempSync(join(tmpdir(), 'locus-mt-fallback-test-'));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const probe = join(root, 'probe');
    const harness = String.raw`
source "$1"
locus() {
  if [[ "$1" == init ]]; then
    printf '%s\n' "$LOCUS_HOME" > "$PROBE_FILE"
    return 1
  fi
}
set +e
mt_grant_probe
exit "$?"
`;
    const env = { ...process.env, PROBE_FILE: probe };
    if (setting === 'empty') env.TMPDIR = '';
    else delete env.TMPDIR;
    const result = spawnSync('bash', ['-c', harness, 'dogfood-test', script], {
      cwd: root, env, encoding: 'utf8',
    });
    assert.equal(result.signal, null);
    assert.equal(result.status, 1);
    const probeRoot = dirname(readFileSync(probe, 'utf8').trim());
    assert.match(probeRoot, /^\/tmp\/locus-mt-dogfood\.[^/]+$/);
    assert.equal(existsSync(probeRoot), false);
  });
}

// The mktemp shim records the template mt_grant_probe passes and redirects
// anything outside $CONTROLLED into it, so the fallback case never touches /tmp.
function mtRootSelection(t, tmpdirEnv) {
  const root = mkdtempSync(join(tmpdir(), 'locus-mt-select-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const controlled = join(root, 'controlled');
  mkdirSync(controlled);
  const template = join(root, 'template');
  const created = join(root, 'created');
  // Plain strings, not String.raw: the shim needs bash's ${...} expansions.
  const harness = [
    'source "$1"',
    'mktemp() {',
    '  local last="${!#}" args=("$@") path',
    '  printf \'%s\\n\' "$last" > "$TEMPLATE_FILE"',
    '  [[ "$last" == "$CONTROLLED"/* ]] || args[$(($# - 1))]="$CONTROLLED/${last##*/}"',
    '  path="$(command mktemp "${args[@]}")" || return',
    '  printf \'%s\\n\' "$path" > "$CREATED_FILE"',
    '  printf \'%s\\n\' "$path"',
    '}',
    'locus() { return 1; }',
    // Match the production caller, which disables errexit for this probe.
    'set +e',
    'mt_grant_probe',
    'exit "$?"',
  ].join('\n');
  const env = { ...process.env, CONTROLLED: controlled, TEMPLATE_FILE: template, CREATED_FILE: created };
  delete env.TMPDIR;
  Object.assign(env, tmpdirEnv(controlled));
  const result = spawnSync('bash', ['-c', harness, 'dogfood-test', script], { cwd: root, env, encoding: 'utf8' });
  assert.equal(result.signal, null);
  assert.equal(result.status, 1, 'stubbed probe stops at the init failure');
  return {
    controlled,
    template: readFileSync(template, 'utf8').trim(),
    created: readFileSync(created, 'utf8').trim(),
    left: readdirSync(controlled),
  };
}

test('multi-tenant probe root is selected and created under an explicit TMPDIR', (t) => {
  const { controlled, template, created, left } = mtRootSelection(t, (dir) => ({ TMPDIR: dir }));
  assert.equal(template, `${controlled}/locus-mt-dogfood.XXXXXX`);
  assert.equal(dirname(created), controlled);
  assert.match(basename(created), /^locus-mt-dogfood\.[A-Za-z0-9]{6}$/);
  assert.equal(existsSync(created), false);
  assert.deepEqual(left, []);
});

test('multi-tenant probe root stays directly beneath an explicit TMPDIR written with a trailing slash', (t) => {
  const { controlled, template, created, left } = mtRootSelection(t, (dir) => ({ TMPDIR: `${dir}/` }));
  assert.equal(template, `${controlled}//locus-mt-dogfood.XXXXXX`);
  assert.equal(resolve(dirname(created)), controlled);
  assert.match(basename(created), /^locus-mt-dogfood\.[A-Za-z0-9]{6}$/);
  assert.equal(existsSync(created), false);
  assert.deepEqual(left, []);
});

for (const [label, tmpdirEnv] of [['unset', () => ({})], ['empty', () => ({ TMPDIR: '' })]]) {
  test(`multi-tenant probe root template falls back to /tmp when TMPDIR is ${label}`, (t) => {
    const { controlled, template, created, left } = mtRootSelection(t, tmpdirEnv);
    assert.equal(template, '/tmp/locus-mt-dogfood.XXXXXX');
    assert.equal(dirname(created), controlled);
    assert.equal(existsSync(created), false);
    assert.deepEqual(left, []);
  });
}

test('multi-tenant probe directory is created literally under a glob-character TMPDIR', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-mt-literal-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, "temporary * [a] 'quoted' directory");
  const neighbor = join(root, "temporary match a 'quoted' directory");
  mkdirSync(scratch);
  mkdirSync(neighbor);
  writeFileSync(join(scratch, 'keep'), 'scratch');
  writeFileSync(join(neighbor, 'keep'), 'neighbor');
  const probe = join(root, 'probe');
  // The probe cds into its root before init, so the stub observes the live
  // directory at creation time: it must sit directly under the literal TMPDIR
  // (exit 43 when it does not), record it, then stop the probe before any
  // grant or server work.
  const harness = String.raw`
source "$1"
locus() {
  if [[ "$1" == init ]]; then
    [[ -d "$PWD" && "${'$'}{PWD%/*}" == "$TMPDIR" && "$PWD" == "$TMPDIR"/locus-mt-dogfood.* && "$LOCUS_HOME" == "$PWD/home" ]] || exit 43
    printf '%s\n' "$PWD" > "$PROBE_FILE"
    return 1
  fi
  return 1
}
set +e
mt_grant_probe
exit "$?"
`;
  const result = spawnSync('bash', ['-c', harness, 'dogfood-test', script], {
    cwd: root,
    env: { ...process.env, TMPDIR: scratch, PROBE_FILE: probe },
    encoding: 'utf8',
  });
  assert.equal(result.signal, null);
  assert.equal(result.status, 1, 'probe must reach init under the supplied TMPDIR and stop on the injected failure');
  const probeRoot = readFileSync(probe, 'utf8').trim();
  assert.match(basename(probeRoot), /^locus-mt-dogfood\.[^/]+$/);
  assert.equal(dirname(probeRoot), scratch);
  assert.equal(existsSync(probeRoot), false);
  assert.deepEqual(readdirSync(scratch), ['keep']);
  assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'scratch');
  assert.equal(readFileSync(join(neighbor, 'keep'), 'utf8'), 'neighbor',
    'a glob-expanded TMPDIR match must not be mistaken for the probe parent');
});

for (const sig of ['HUP']) {
  test(`dogfood home under TMPDIR is removed when the run receives SIG${sig}`, (t) => {
    const root = mkdtempSync(join(tmpdir(), 'locus-dogfood-signal-'));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const scratch = join(root, 'temporary directory');
    const fixtureRoot = join(root, 'fixture');
    const bin = join(fixtureRoot, 'target', 'debug');
    mkdirSync(scratch);
    mkdirSync(bin, { recursive: true });
    mkdirSync(join(fixtureRoot, 'scripts'));
    writeFileSync(join(scratch, 'keep'), 'preserve');
    writeFileSync(join(fixtureRoot, 'scripts', 'dogfood.sh'), readFileSync(script));
    writeFileSync(join(bin, 'cargo'), '#!/bin/sh\nexit 0\n', { mode: 0o755 });
    writeFileSync(join(bin, 'locus-mcp'), '#!/bin/sh\nexit 0\n', { mode: 0o755 });
    // quickstart signals the parent script while the home exists, then lets it unwind.
    writeFileSync(join(bin, 'locus'), `#!/bin/bash
case "$1" in
  init) printf '%s\\n' "$LOCUS_HOME" > "$PROBE_FILE" ;;
  quickstart) kill -${sig} "$PPID"; sleep 1; exit 0 ;;
esac
`, { mode: 0o755 });
    const probe = join(root, 'probe');
    const result = spawnSync('bash', [join(fixtureRoot, 'scripts', 'dogfood.sh')], {
      cwd: root,
      env: { PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: scratch, PROBE_FILE: probe },
      encoding: 'utf8',
      timeout: 30_000,
    });

    assert.ifError(result.error);
    const home = readFileSync(probe, 'utf8').trim();
    assert.equal(dirname(home), scratch);
    assert.match(basename(home), /^locus-dogfood\./);
    assert.equal(existsSync(home), false, 'signal must not leave the dogfood home behind');
    assert.deepEqual(readdirSync(scratch), ['keep']);
  });
}

// Unlike the stubbed fallback cases above, nothing is redirected here: the
// real mktemp runs, so the directory the script creates is the one checked.
test('dogfood home is really created beneath /tmp when TMPDIR is unset', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-dogfood-real-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const fixtureRoot = join(root, 'fixture');
  const bin = join(fixtureRoot, 'target', 'debug');
  mkdirSync(join(fixtureRoot, 'scripts'), { recursive: true });
  mkdirSync(bin, { recursive: true });
  writeFileSync(join(fixtureRoot, 'scripts', 'dogfood.sh'), readFileSync(script));
  writeFileSync(join(bin, 'cargo'), '#!/bin/sh\nexit 0\n', { mode: 0o755 });
  writeFileSync(join(bin, 'locus-mcp'), '#!/bin/sh\nexit 0\n', { mode: 0o755 });
  const probe = join(root, 'probe');
  writeFileSync(join(bin, 'locus'), `#!/bin/bash
case "$1" in
  init) printf '%s\\n' "$LOCUS_HOME" > "$PROBE_FILE"; [[ -d "$LOCUS_HOME" ]] || exit 88 ;;
  quickstart) exit 42 ;;
esac
`, { mode: 0o755 });
  const env = { PATH: process.env.PATH, HOME: process.env.HOME, PROBE_FILE: probe };
  const result = spawnSync('bash', [join(fixtureRoot, 'scripts', 'dogfood.sh')], {
    cwd: root, env, encoding: 'utf8',
  });
  const home = readFileSync(probe, 'utf8').trim();
  t.after(() => rmSync(home, { recursive: true, force: true }));
  assert.equal(result.status, 42);
  assert.equal(dirname(home), '/tmp');
  assert.match(basename(home), /^locus-dogfood\.[A-Za-z0-9]{6}$/);
  assert.equal(existsSync(home), false, 'cleanup trap removes the home');
});

test('multi-tenant probe directory is created as one path under a TMPDIR containing spaces', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-mt-space-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  // Double and trailing spaces would each split into extra mktemp arguments if
  // the template were ever unquoted.
  const scratch = join(root, 'tmp  with   spaces ');
  mkdirSync(scratch);
  const probe = join(root, 'probe');
  // Observe the live directory from inside init: it must exist, be a single
  // child of the exact TMPDIR, and hold LOCUS_HOME.
  const harness = String.raw`
source "$1"
locus() {
  if [[ "$1" == init ]]; then
    [[ -d "$PWD" && "${'$'}{PWD%/*}" == "$TMPDIR" && "$PWD" == "$TMPDIR"/locus-mt-dogfood.* && "$LOCUS_HOME" == "$PWD/home" ]] || exit 43
    printf '%s\n' "$PWD" > "$PROBE_FILE"
  fi
  return 1
}
set +e
mt_grant_probe
exit "$?"
`;
  const result = spawnSync('bash', ['-c', harness, 'dogfood-test', script], {
    cwd: root,
    env: { ...process.env, TMPDIR: scratch, PROBE_FILE: probe },
    encoding: 'utf8',
  });
  assert.equal(result.signal, null);
  assert.equal(result.status, 1, 'probe must reach init under the spaced TMPDIR');
  const probeRoot = readFileSync(probe, 'utf8').trim();
  assert.equal(dirname(probeRoot), scratch);
  assert.match(basename(probeRoot), /^locus-mt-dogfood\.[A-Za-z0-9]{6}$/);
  assert.equal(existsSync(probeRoot), false, 'failure path removes the probe root');
  assert.deepEqual(readdirSync(scratch), []);
  assert.deepEqual(readdirSync(root).sort(), ['probe', 'tmp  with   spaces ']);
});
