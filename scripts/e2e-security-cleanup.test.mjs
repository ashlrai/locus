// Run: node --test scripts/e2e-security-cleanup.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { chmodSync, existsSync, lstatSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const source = readFileSync(join(root, 'scripts', 'e2e.sh'), 'utf8');

// Real pieces of e2e.sh: the exit trap, the two creators, and the opening of
// section 24 (creation, containment check, first binding probes). A recording
// mktemp delegates to the real allocator; only the locus binary is a stub.
// A probe failure goes through the script's own die() and EXIT trap.
function piece(re, what) {
  const m = source.match(re)?.[0];
  assert.ok(m, `e2e.sh must contain ${what}`);
  return m;
}
function section24Head() {
  const start = source.indexOf('log "24.');
  const endMarker = 'ok "binding add rejects conflicting content and preserves the existing file"';
  const end = source.indexOf(endMarker, start);
  assert.ok(start >= 0 && end > start, 'e2e.sh must contain the section 24 binding probes');
  return source.slice(start, end + endMarker.length);
}

// Stub locus: `binding add` is idempotent for identical input and rejects a
// different account, unless STUB_MODE=mutate, where every add rewrites the
// file so the "identical retry changed existing file" probe must fail.
const STUB = `#!/usr/bin/env bash
case "$1" in
  init) mkdir -p "$LOCUS_HOME/bindings" ;;
  binding)
    f="$LOCUS_HOME/bindings/conflict-probe.toml"
    acct=""; while [[ $# -gt 0 ]]; do [[ "$1" == --account ]] && acct="$2"; shift; done
    if [[ "\${STUB_MODE:-}" == mutate ]]; then date +%s%N > "$f"; exit 0; fi
    if [[ ! -e "$f" ]]; then echo "$acct" > "$f"
    elif [[ "$(cat "$f")" != "$acct" ]]; then echo "binding 'conflict-probe' already exists" >&2; exit 1
    fi ;;
esac
`;

function run(t, mode) {
  const dir = mkdtempSync(join(tmpdir(), 'locus-e2e-security-cleanup-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const scratch = join(dir, 'scratch with  spaces');
  const mainHome = join(dir, 'main-home');
  const stub = join(dir, 'locus-stub');
  const seen = join(dir, 'seen');
  mkdirSync(scratch);
  mkdirSync(mainHome);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  writeFileSync(stub, STUB);
  chmodSync(stub, 0o755);
  const script = `set -euo pipefail
mktemp() {
  local created
  created="$(command mktemp "$@")" || return $?
  [[ -d "$created" ]] || return 72
  touch "$created/cleanup-sentinel"
  printf '%s\\n' "$created" >> "$SEEN"
  printf '%s\\n' "$created"
}
${piece(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m, 'e2e_cleanup')}
${piece(/^create_security_home\(\) \{[\s\S]*?^\}/m, 'create_security_home')}
${piece(/^create_security_workspace\(\) \{[\s\S]*?^\}/m, 'create_security_workspace')}
log() { :; }
ok() { printf 'ok %s\\n' "$*"; }
die() { printf 'FAIL %s\\n' "$*" >&2; exit 1; }
LOCUS_BIN="$STUB"
LOCUS_HOME="$MAIN_HOME"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
trap 'e2e_cleanup' EXIT
${section24Head()}
`;
  const result = spawnSync('bash', ['-c', script], {
    env: { ...process.env, TMPDIR: scratch, MAIN_HOME: mainHome, STUB: stub, SEEN: seen, STUB_MODE: mode },
    encoding: 'utf8',
    timeout: 30_000,
  });
  return { scratch, mainHome, seen, result };
}

function assertSecurityDirectoriesRemoved({ scratch, mainHome, seen, result }) {
  assert.ifError(result.error);
  assert.equal(result.signal, null, 'the shell must exit normally');
  const paths = readFileSync(seen, 'utf8').trimEnd().split('\n');
  assert.equal(paths.length, 2, 'both security allocations must be recorded');
  const [home, ws] = paths;
  assert.notEqual(home, ws, 'security home and workspace must be distinct');
  assert.equal(dirname(home), scratch, 'security home must be directly beneath TMPDIR');
  assert.equal(dirname(ws), scratch, 'security workspace must be directly beneath TMPDIR');
  assert.match(home, /\/locus-security-e2e\.[^/]+$/);
  assert.match(ws, /\/locus-security-ws\.[^/]+$/);
  for (const path of paths) {
    assert.throws(() => lstatSync(path), { code: 'ENOENT' },
      'each populated security directory must be removed on exit');
  }
  assert.deepEqual(readdirSync(scratch), ['keep'], 'cleanup must preserve unrelated files');
  assert.ok(!existsSync(mainHome), 'the exit trap also removes the main home');
}

test('security home and workspace are removed when section 24 passes', (t) => {
  const observed = run(t, 'ok');
  const { result } = observed;
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /ok binding add rejects conflicting content/);
  assertSecurityDirectoriesRemoved(observed);
});

test('security home and workspace are removed when a security probe fails', (t) => {
  const observed = run(t, 'mutate');
  const { result } = observed;
  assert.equal(result.status, 1, 'the failed probe must fail the suite');
  assert.match(result.stderr, /FAIL identical binding retry changed existing file/);
  assertSecurityDirectoriesRemoved(observed);
});

test('security home creation failure removes all previously allocated temporary paths', (t) => {
  const dir = mkdtempSync(join(tmpdir(), 'locus-e2e-security-failure-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const scratch = join(dir, 'scratch with spaces');
  const seen = join(dir, 'seen');
  const attempts = join(dir, 'attempts');
  const probe = join(dir, 'probe');
  const stub = join(dir, 'locus-stub');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  writeFileSync(stub, '#!/usr/bin/env bash\ntouch "$PROBE"\n');
  chmodSync(stub, 0o755);
  // Use the real creators for populated earlier fixtures; fail only the
  // security-home allocation. No cargo or provider is called.
  const script = `set -euo pipefail
${piece(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m, 'e2e_cleanup')}
${piece(/^e2e_install_traps\(\) \{[\s\S]*?^\}/m, 'e2e_install_traps')}
mktemp() {
  printf '%s\\n' "\${2:-}" >> "$ATTEMPTS"
  if [[ "\${2:-}" == */locus-security-e2e.XXXXXX ]]; then
    while IFS= read -r path; do
      [[ -f "$path/cleanup-sentinel" ]] || return 74
    done < "$SEEN"
    return 73
  fi
  local created
  created="$(command mktemp "$@")" || return $?
  touch "$created/cleanup-sentinel"
  printf '%s\\n' "$created" >> "$SEEN"
  printf '%s\\n' "$created"
}
${piece(/^create_locus_home\(\) \{[\s\S]*?^\}/m, 'create_locus_home')}
${piece(/^create_workspace\(\) \{[\s\S]*?^\}/m, 'create_workspace')}
${piece(/^create_graph_home\(\) \{[\s\S]*?^\}/m, 'create_graph_home')}
${piece(/^create_security_home\(\) \{[\s\S]*?^\}/m, 'create_security_home')}
${piece(/^create_security_workspace\(\) \{[\s\S]*?^\}/m, 'create_security_workspace')}
log() { :; }
ok() { :; }
die() { exit 1; }
LOCUS_BIN="$STUB"
WS_DIR="" GRAPH_HOME="" GRAPH_HOMES="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
create_locus_home
e2e_install_traps
create_workspace
create_graph_home
${section24Head()}
`;
  const result = spawnSync('bash', ['-c', script], {
    env: { ...process.env, TMPDIR: scratch, STUB: stub, ATTEMPTS: attempts, PROBE: probe, SEEN: seen },
    encoding: 'utf8', timeout: 30_000,
  });
  assert.ifError(result.error);
  assert.equal(result.signal, null);
  assert.equal(result.status, 73, 'the allocation failure must fail the suite');
  const prefixes = ['locus-e2e.', 'locus-ws.', 'locus-graph-imp.'];
  assert.deepEqual(readFileSync(attempts, 'utf8').trim().split('\n'),
    [...prefixes, 'locus-security-e2e.'].map((prefix) => join(scratch, `${prefix}XXXXXX`)),
    'only security-home creation must fail; security-workspace allocation must not run');
  const paths = readFileSync(seen, 'utf8').trim().split('\n');
  assert.equal(paths.length, prefixes.length, 'all three earlier allocations must succeed');
  for (const [index, path] of paths.entries()) {
    assert.equal(dirname(path), scratch);
    assert.ok(path.startsWith(join(scratch, prefixes[index])));
    assert.throws(() => lstatSync(path), { code: 'ENOENT' },
      'each previously populated temporary directory must be removed');
  }
  assert.ok(!existsSync(probe), 'the security checks must not run');
  assert.deepEqual(readdirSync(scratch), ['keep'], 'cleanup must preserve unrelated files');
  assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'unrelated');
});

test('security workspace creation failure removes the allocated security home', (t) => {
  const dir = mkdtempSync(join(tmpdir(), 'locus-e2e-security-ws-failure-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const scratch = join(dir, 'scratch with spaces');
  const mainHome = join(dir, 'main-home');
  const seen = join(dir, 'seen');
  const attempts = join(dir, 'attempts');
  const probe = join(dir, 'probe');
  const stub = join(dir, 'locus-stub');
  mkdirSync(scratch);
  mkdirSync(mainHome);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  writeFileSync(stub, '#!/usr/bin/env bash\ntouch "$PROBE"\n');
  chmodSync(stub, 0o755);
  const script = `set -euo pipefail
${piece(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m, 'e2e_cleanup')}
mktemp() {
  printf '%s\\n' "\${2:-}" >> "$ATTEMPTS"
  if [[ "\${2:-}" == */locus-security-ws.XXXXXX ]]; then
    [[ -d "$SECURITY_HOME" ]] || exit 74
    touch "$SECURITY_HOME/cleanup-sentinel"
    printf '%s' "$SECURITY_HOME" > "$SEEN"
    return 73
  fi
  command mktemp "$@"
}
${piece(/^create_security_home\(\) \{[\s\S]*?^\}/m, 'create_security_home')}
${piece(/^create_security_workspace\(\) \{[\s\S]*?^\}/m, 'create_security_workspace')}
log() { :; }
ok() { :; }
die() { exit 1; }
LOCUS_BIN="$STUB"
LOCUS_HOME="$MAIN_HOME"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
${piece(/^  trap 'e2e_cleanup' EXIT$/m, 'the EXIT cleanup trap')}
${section24Head()}
`;
  const result = spawnSync('bash', ['-c', script], {
    env: { ...process.env, TMPDIR: scratch, MAIN_HOME: mainHome, STUB: stub,
      ATTEMPTS: attempts, SEEN: seen, PROBE: probe },
    encoding: 'utf8', timeout: 30_000,
  });
  assert.ifError(result.error);
  assert.equal(result.signal, null);
  assert.equal(result.status, 73, 'workspace allocation failure must fail the suite');
  assert.deepEqual(readFileSync(attempts, 'utf8').trim().split('\n'), [
    join(scratch, 'locus-security-e2e.XXXXXX'),
    join(scratch, 'locus-security-ws.XXXXXX'),
  ]);
  const home = readFileSync(seen, 'utf8');
  assert.equal(dirname(home), scratch);
  assert.match(home, /\/locus-security-e2e\.[^/]+$/);
  assert.ok(!existsSync(home), 'the allocated SECURITY_HOME must be removed on failure');
  assert.deepEqual(readdirSync(scratch), ['keep'], 'cleanup must preserve unrelated files');
  assert.ok(!existsSync(probe), 'security checks must not run after allocation failure');
  assert.ok(!existsSync(mainHome), 'the exit trap must also remove the main home');
});
