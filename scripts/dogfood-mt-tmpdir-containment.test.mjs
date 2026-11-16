// Run: node --test scripts/dogfood-mt-tmpdir-containment.test.mjs
// The multi-tenant grant probe (step 8b, DOGFOOD_MT=1) allocates probe_root
// under ${TMPDIR:-/tmp}: a supplied, nonempty TMPDIR is the containment
// boundary the caller chose, and probe_root must live inside it as a real
// directory entry — alongside the dogfood home when both are live, without
// drifting to TMP, TEMP or the working directory, and failing closed with no
// /tmp retry when the supplied root does not exist. The end-to-end probe runs
// the script's real mainline; fixture stubs provide cargo, locus-mcp (a real
// local HTTP server) and a recording locus.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'dogfood.sh');

// A dogfood checkout whose every external command is stubbed: the locus stub
// drives the whole mainline to readiness and answers the probe's grant
// lifecycle from state on disk; the locus-mcp stub is a real local server so
// the probe's health, tenantless-401, granted-200 and revoked-401 checks run
// against live HTTP.
function fixture(root) {
  const fixtureRoot = join(root, 'fixture');
  const bin = join(fixtureRoot, 'target', 'debug');
  mkdirSync(bin, { recursive: true });
  mkdirSync(join(fixtureRoot, 'scripts'));
  writeFileSync(join(fixtureRoot, 'scripts', 'dogfood.sh'), readFileSync(script));
  writeFileSync(join(fixtureRoot, 'scripts', 'dogfood-ready.jq'), readFileSync(join(dirname(script), 'dogfood-ready.jq')));
  writeFileSync(join(fixtureRoot, 'scripts', 'hub-smoke.sh'), '#!/bin/sh\nexit 0\n');
  writeFileSync(join(bin, 'cargo'), '#!/bin/sh\nexit 0\n', { mode: 0o755 });
  writeFileSync(join(bin, 'uname'), '#!/bin/sh\nprintf Darwin\n', { mode: 0o755 });
  writeFileSync(join(bin, 'locus-mcp'), String.raw`#!/usr/bin/env node
const http = require('node:http');
const fs = require('node:fs');
if (process.argv[2] === '--help') {
  process.stdout.write('usage: locus-mcp [--http <addr> --multi-tenant]\n');
  process.exit(0);
}
if (process.env.MT_FAIL_MARKER && fs.existsSync(process.env.MT_FAIL_MARKER)) fs.writeFileSync(process.env.MT_AFTER_FAILURE, 'server');
if (process.argv[2] !== '--http' || process.argv[4] !== '--multi-tenant') process.exit(2);
const [host, port] = process.argv[3].split(':');
const server = http.createServer((req, res) => {
  if (req.method === 'GET' && req.url === '/health') { res.writeHead(200).end(); return; }
  let body = '';
  req.on('data', (c) => { body += c; });
  req.on('end', () => {
    try { JSON.parse(body); } catch { res.writeHead(400).end(); return; }
    const token = req.headers['x-locus-tenant-token'];
    const revoked = fs.existsSync(process.env.MT_STATE_DIR + '/revoked');
    if (req.headers.authorization === 'Bearer ' + process.env.LOCUS_MCP_HTTP_TOKEN &&
        token === 'lmt_fixture' && !revoked) {
      res.writeHead(200).end('{}');
      return;
    }
    res.writeHead(401).end('invalid_grant');
  });
});
server.listen(Number(port), host, () => fs.writeFileSync(process.env.MT_PID_FILE, String(process.pid)));
`, { mode: 0o755 });

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

  writeFileSync(join(bin, 'locus'), `#!/usr/bin/env bash
if [[ -n "\${MT_FAIL_MARKER:-}" && -f "$MT_FAIL_MARKER" ]]; then touch "$MT_AFTER_FAILURE"; fi
case "$1 $2" in
  'init ')
    if [[ "$LOCUS_HOME" == */locus-mt-dogfood.*/home && "$PWD" == "\${LOCUS_HOME%/home}" ]]; then
      # Step 8b probe: it cd'd into probe_root, so PWD is the root itself —
      # it must be a real, unsymlinked directory directly beneath TMPDIR,
      # sharing that boundary with the live dogfood home and nothing else.
      [[ -d "$PWD" && ! -L "$PWD" && "\${PWD%/*}" == "$TMPDIR" ]] || exit 71
      [[ "$PWD" == "$TMPDIR"/locus-mt-dogfood.* && "$LOCUS_HOME" == "$PWD/home" ]] || exit 72
      [[ "$(find "$TMPDIR" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')" == 2 ]] || exit 73
      ls -A "$TMPDIR" | sort > "$MT_LISTING"
      printf '%s\\n' "$PWD" > "$MT_PROBE_FILE"
      mkdir -p "$LOCUS_HOME"
    elif [[ "$LOCUS_HOME" == "$TMPDIR"/locus-dogfood.* && "$PWD" == "$LOCUS_HOME/project" ]]; then
      printf '%s\\n' "$LOCUS_HOME" > "$PROBE_FILE"
    fi ;;
  'binding add') mkdir -p "$LOCUS_HOME/bindings"; touch "$LOCUS_HOME/bindings/$3.toml" ;;
  'exec --no-resolve') printf '%s\\n' '{"id":2,"result":{"tools":[]}}' '{"id":3,"result":{"isError":false,"content":[{"text":"{\\"isError\\":false,\\"content\\":[{\\"text\\":\\"sandboxed=1 backend=sandbox-exec\\"}]}"}]}}' ;;
  'agent report') cat "$FIXTURE_ROOT/report.json" ;;
  'verify session') [[ "\${3:-}" == --help ]] || cat "$FIXTURE_ROOT/verify.json" ;;
  'forensics export') printf '{}\\n' > "$4" ;;
  'goal status') exit 1 ;;
  'mcp mint') [[ "\${3:-}" != --help ]] || exit 0; [[ -f "$LOCUS_HOME/bindings/mt-probe.toml" ]] || exit 76; touch "$MT_STATE_DIR/mint-attempted"; [[ "\${MT_FAIL_MINT:-0}" != 1 ]] || exit 75; printf '%s\\n' '{"grant_id":"grant_fixture","token":"lmt_fixture"}' ;;
  'mcp list')
    if [[ -f "$MT_STATE_DIR/revoked" ]]; then printf '[]\\n'
    else printf '%s\\n' '[{"grant_id":"grant_fixture","revoked":false,"expired":false}]'; fi ;;
  'mcp revoke') [[ "$3" == grant_fixture ]] || exit 74; touch "$MT_STATE_DIR/revoked" ;;
  '--version '|'quickstart '|'pin sandbox-probe'|'agent setup'|'doctor ') ;;
  *) exit 90 ;;
esac
`, { mode: 0o755 });
  return join(fixtureRoot, 'scripts', 'dogfood.sh');
}

test('a controlled TMPDIR contains the multi-tenant probe root during a dogfood run', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-mt-containment-'));
  const pidFile = join(root, 'mt-server-pid');
  t.after(() => {
    if (existsSync(pidFile)) {
      try { process.kill(Number(readFileSync(pidFile, 'utf8').trim())); }
      catch (error) { if (error.code !== 'ESRCH') throw error; }
    }
    rmSync(root, { recursive: true, force: true });
  });
  const scratch = join(root, 'controlled tmp root');
  const tmpDecoy = join(root, 'TMP decoy');
  const tempDecoy = join(root, 'TEMP decoy');
  const cwdDecoy = join(root, 'cwd decoy');
  for (const dir of [scratch, tmpDecoy, tempDecoy, cwdDecoy]) mkdirSync(dir);
  writeFileSync(join(scratch, 'keep'), 'unrelated');

  const probe = join(root, 'probe');
  const mtProbe = join(root, 'mt-probe');
  const mtListing = join(root, 'mt-listing');
  const dogfood = fixture(root);
  const result = spawnSync('bash', [dogfood], {
    cwd: cwdDecoy,
    env: { ...process.env, TMPDIR: scratch, TMP: tmpDecoy, TEMP: tempDecoy,
      DOGFOOD_MT: '1', PROBE_FILE: probe, MT_PROBE_FILE: mtProbe,
      MT_LISTING: mtListing, MT_STATE_DIR: root, MT_PID_FILE: pidFile,
      FIXTURE_ROOT: root },
    encoding: 'utf8',
    timeout: 60_000,
  });

  assert.ifError(result.error);
  assert.equal(result.signal, null);
  assert.equal(result.status, 0, `the run must reach DOGFOOD READY: ${result.stderr}`);
  assert.match(result.stdout, /multi-tenant probe/);
  assert.match(result.stdout, /DOGFOOD READY/);
  const home = readFileSync(probe, 'utf8').trim();
  const probeRoot = readFileSync(mtProbe, 'utf8').trim();
  assert.equal(dirname(probeRoot), scratch);
  assert.match(basename(probeRoot), /^locus-mt-dogfood\.[^/]+$/,
    'the leaf is one mktemp component beneath the controlled root');
  assert.deepEqual(readFileSync(mtListing, 'utf8').trim().split('\n').sort(),
    ['keep', basename(home), basename(probeRoot)].sort(),
    'while live, TMPDIR holds only the dogfood home, the probe root and unrelated files');
  assert.equal(existsSync(probeRoot), false, 'the probe root is removed with its home');
  assert.equal(existsSync(home), false);
  assert.deepEqual(readdirSync(scratch), ['keep'],
    'cleanup preserves unrelated files in the controlled root');
  for (const decoy of [tmpDecoy, tempDecoy, cwdDecoy]) {
    assert.deepEqual(readdirSync(decoy), [],
      `${basename(decoy)} must not receive the probe root`);
  }
  const pid = Number(readFileSync(pidFile, 'utf8').trim());
  assert.ok(Number.isInteger(pid) && pid > 0);
  assert.throws(() => process.kill(pid, 0), { code: 'ESRCH' },
    'the exit trap must reap the probe server');
});

test('a failed multi-tenant mint removes the populated probe root', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-mt-failure-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'controlled tmp root');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  const mtProbe = join(root, 'mt-probe');
  const result = spawnSync('bash', [fixture(root)], {
    cwd: root,
    env: { ...process.env, TMPDIR: scratch, DOGFOOD_MT: '1',
      PROBE_FILE: join(root, 'probe'), MT_PROBE_FILE: mtProbe,
      MT_LISTING: join(root, 'mt-listing'), MT_STATE_DIR: root,
      MT_PID_FILE: join(root, 'mt-server-pid'), FIXTURE_ROOT: root, MT_FAIL_MINT: '1' },
    encoding: 'utf8',
    timeout: 60_000,
  });

  assert.ifError(result.error);
  assert.equal(result.signal, null);
  assert.notEqual(result.status, 0, 'a failed mint must fail dogfood');
  assert.match(result.stderr, /mt probe: mint failed/);
  assert.match(result.stderr, /multi-tenant probe failed/);
  assert.doesNotMatch(result.stdout, /DOGFOOD READY/);
  assert.equal(existsSync(join(root, 'mint-attempted')), true, 'the injected failure must be reached');
  const probeRoot = readFileSync(mtProbe, 'utf8').trim();
  assert.equal(dirname(probeRoot), scratch);
  assert.match(basename(probeRoot), /^locus-mt-dogfood\.[^/]+$/);
  assert.equal(existsSync(probeRoot), false, 'the failed probe must remove its root');
  assert.deepEqual(readdirSync(scratch), ['keep'], 'cleanup preserves unrelated files');
});

// TMP, TEMP and the working directory are all plausible but wrong roots: the
// probe must read TMPDIR alone when it chooses where probe_root goes.
test('multi-tenant probe root selection ignores TMP, TEMP and the working directory', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-mt-decoys-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'controlled root');
  const tmpDecoy = join(root, 'TMP decoy');
  const tempDecoy = join(root, 'TEMP decoy');
  const cwdDecoy = join(root, 'cwd decoy');
  for (const dir of [scratch, tmpDecoy, tempDecoy, cwdDecoy]) mkdirSync(dir);

  const probe = join(root, 'probe');
  // The probe cds into its root before init, so the stub observes the live
  // directory: it must sit directly under TMPDIR (exit 71 when it does not),
  // record it, then stop the probe before any grant or server work.
  const harness = String.raw`
source "$1"
locus() {
  if [[ "$1" == init ]]; then
    [[ -d "$PWD" && ! -L "$PWD" && "${'$'}{PWD%/*}" == "$TMPDIR" && "$PWD" == "$TMPDIR"/locus-mt-dogfood.* ]] || exit 71
    printf '%s\n' "$PWD" > "$PROBE_FILE"
  fi
  return 1
}
set +e
mt_grant_probe
exit "$?"
`;
  const result = spawnSync('bash', ['-c', harness, 'dogfood-test', script], {
    cwd: cwdDecoy,
    env: { ...process.env, TMPDIR: scratch, TMP: tmpDecoy, TEMP: tempDecoy, PROBE_FILE: probe },
    encoding: 'utf8',
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 1, `selection must succeed and stop at the injected failure: ${result.stderr}`);
  const probeRoot = readFileSync(probe, 'utf8').trim();
  assert.equal(dirname(probeRoot), scratch,
    'TMPDIR — not TMP, TEMP or the working directory — is the selected root');
  assert.equal(existsSync(probeRoot), false);
  assert.deepEqual(readdirSync(scratch), []);
  for (const decoy of [tmpDecoy, tempDecoy, cwdDecoy]) {
    assert.deepEqual(readdirSync(decoy), [],
      `${basename(decoy)} must not receive the probe root`);
  }
});

// A supplied TMPDIR is the boundary the caller chose: when it does not exist
// the probe must stop, not silently substitute /tmp or any other root. The
// recording mktemp proves the supplied template is the only request the
// probe ever makes.
test('a supplied TMPDIR that does not exist fails the multi-tenant probe without a /tmp fallback', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-mt-missing-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const missing = join(root, 'missing root');
  const controlled = join(root, 'controlled');
  mkdirSync(controlled);
  const templates = join(root, 'templates');
  const locusCalled = join(root, 'locus-called');

  const harness = String.raw`
source "$1"
mktemp() {
  local last="${'$'}{!#}"
  printf '%s\n' "$last" >> "$MKTEMP_LOG"
  command mktemp "$@"
}
locus() { touch "$LOCUS_CALLED"; }
set +e
mt_grant_probe
exit "$?"
`;
  const result = spawnSync('bash', ['-c', harness, 'dogfood-test', script], {
    cwd: root,
    env: { ...process.env, TMPDIR: missing, MKTEMP_LOG: templates, LOCUS_CALLED: locusCalled },
    encoding: 'utf8',
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 1, 'a missing controlled root must stop the probe');
  assert.match(result.stderr, /mktemp/);
  assert.equal(readFileSync(templates, 'utf8').trim(), `${missing}/locus-mt-dogfood.XXXXXX`,
    'the probe must request the supplied root only — never retry beneath /tmp');
  assert.equal(existsSync(locusCalled), false, 'locus must not run after allocation fails');
  assert.equal(existsSync(missing), false, 'the missing root must not be created for it');
  assert.deepEqual(readdirSync(controlled), [],
    'no fallback directory may be allocated beneath another root');
});

// Exercise the mainline's hard failure when only the step 8b allocation fails.
test('dogfood stops before multi-tenant execution when probe allocation fails', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-mt-allocation-failure-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'scratch');
  mkdirSync(scratch);
  const dogfood = fixture(root);
  const failure = join(root, 'allocation-failed');
  const afterFailure = join(root, 'after-failure');
  const attempts = join(root, 'allocation-attempts');
  writeFileSync(join(root, 'fixture', 'target', 'debug', 'mktemp'), `#!/bin/bash
if [[ "\${!#}" == */locus-mt-dogfood.XXXXXX ]]; then
  printf '%s\\n' "\${!#}" >> "$MT_ATTEMPTS"
  touch "$MT_FAIL_MARKER"
  exit 1
fi
exec /usr/bin/mktemp "$@"
`, { mode: 0o755 });
  const result = spawnSync('bash', [dogfood], {
    cwd: root,
    env: { ...process.env, TMPDIR: scratch, DOGFOOD_MT: '1',
      PROBE_FILE: join(root, 'probe'), MT_PROBE_FILE: join(root, 'mt-probe'),
      MT_LISTING: join(root, 'mt-listing'), MT_STATE_DIR: root,
      MT_PID_FILE: join(root, 'mt-server-pid'), FIXTURE_ROOT: root,
      MT_FAIL_MARKER: failure, MT_AFTER_FAILURE: afterFailure, MT_ATTEMPTS: attempts },
    encoding: 'utf8', timeout: 60_000,
  });

  assert.ifError(result.error);
  assert.equal(result.signal, null);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /multi-tenant probe failed/);
  assert.doesNotMatch(result.stdout, /DOGFOOD READY/);
  assert.equal(readFileSync(attempts, 'utf8').trim(), `${scratch}/locus-mt-dogfood.XXXXXX`);
  assert.equal(existsSync(afterFailure), false, 'no locus command or multi-tenant server may run after allocation fails');
  assert.equal(existsSync(join(root, 'mt-probe')), false);
  assert.equal(existsSync(join(root, 'mint-attempted')), false);
  assert.equal(existsSync(join(root, 'mt-server-pid')), false);
  assert.deepEqual(readdirSync(scratch), []);
});
