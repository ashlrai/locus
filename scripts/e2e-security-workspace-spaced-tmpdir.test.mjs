// Run: node --test scripts/e2e-security-workspace-spaced-tmpdir.test.mjs
// SECURITY_WS is the working directory the broken-link security probes cd
// into. With a space-containing TMPDIR the allocation must land directly
// beneath that root as one unsplit path, stay usable (nested writes, links,
// entry), be private to the owner, and be removed by the exit trap — without
// touching the supplied root or a sibling at the root's first word-split
// fragment. These tests run the script's own create_security_workspace and
// e2e_cleanup verbatim; a doctored cleanup proves the guard distinguishes an
// unquoted-rm regression.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, lstatSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'e2e.sh');

function lifecycle() {
  const source = readFileSync(script, 'utf8');
  const cleanup = source.match(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m)?.[0];
  const create = source.match(/^create_security_workspace\(\) \{[\s\S]*?^\}/m)?.[0];
  assert.ok(cleanup && create, 'e2e.sh must define the security workspace lifecycle');
  return { cleanup, create };
}

// The temporary root starts with the word "security" followed by a doubled
// space, so a sibling named "security" is exactly where an unquoted expansion
// of the workspace path would strike first.
function fixture(t) {
  const root = mkdtempSync(join(tmpdir(), 'locus-secws-space-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const decoy = join(root, 'security');
  const scratch = join(root, 'security  ws tmp root');
  const observed = join(root, 'workspace-path');
  const mode = join(root, 'mode');
  mkdirSync(decoy);
  mkdirSync(scratch);
  writeFileSync(join(decoy, 'decoy-sentinel'), 'decoy');
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  return { root, decoy, scratch, observed, mode };
}

function probe(f, body, { buggy = false } = {}) {
  const { cleanup, create } = lifecycle();
  // With `buggy`, the later definition wins: the quoting regression the guard
  // must catch. Every other variable stays quoted so only SECURITY_WS splits.
  const override = buggy
    ? 'e2e_cleanup() { rm -rf "$LOCUS_HOME" "${WS_DIR:-}" "${GRAPH_HOME:-}" "${SECURITY_HOME:-}" ${SECURITY_WS:-}; }'
    : '';
  return spawnSync('bash', ['-c', `set -euo pipefail
${cleanup}
${create}
${override}
TMPDIR="$1"
LOCUS_HOME="$TMPDIR/main-home"
mkdir "$LOCUS_HOME"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
trap 'e2e_cleanup' EXIT
create_security_workspace
${body}
`, 'security-ws-spaced-tmpdir', f.scratch], {
    cwd: f.root,
    env: { ...process.env, TMPDIR: f.scratch, OBSERVED: f.observed, MODE_RECORD: f.mode },
    encoding: 'utf8',
  });
}

test('the allocated security workspace is usable and removed on exit beneath a spaced TMPDIR', (t) => {
  const f = fixture(t);
  const result = probe(f, `[[ -d "$SECURITY_WS" && "\${SECURITY_WS%/*}" == "$TMPDIR" && \\
  "\${SECURITY_WS##*/}" == locus-security-ws.* ]] || exit 71
{ stat -f %Lp "$SECURITY_WS" 2>/dev/null || stat -c %a "$SECURITY_WS"; } > "$MODE_RECORD"
# Usable, not just present: nested spaced paths round-trip, a policy link
# lands inside it (as the real probes require), and entering it resolves to
# one physical path beneath the spaced root.
mkdir -p "$SECURITY_WS/probe dir" || exit 72
printf 'payload' > "$SECURITY_WS/probe dir/f.toml" || exit 73
[[ "$(cat "$SECURITY_WS/probe dir/f.toml")" == payload ]] || exit 74
ln -s missing-policy.toml "$SECURITY_WS/.locus.toml" || exit 75
[[ -L "$SECURITY_WS/.locus.toml" ]] || exit 76
[[ "$(cd "$SECURITY_WS" && pwd -P)" == "$(cd "$TMPDIR" && pwd -P)/\${SECURITY_WS##*/}" ]] || exit 77
touch "$SECURITY_WS/cleanup-sentinel"
printf '%s' "$SECURITY_WS" > "$OBSERVED"
`);

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, `probe failed: ${result.stderr}`);
  const workspace = readFileSync(f.observed, 'utf8');
  assert.equal(dirname(workspace), f.scratch, 'allocated directly beneath the spaced TMPDIR');
  assert.match(workspace, /\/locus-security-ws\.[^/\s]+$/, 'the workspace name is one unsplit component');
  assert.ok(workspace.includes(' '), 'the allocated path must contain spaces for this test to mean anything');
  assert.equal(readFileSync(f.mode, 'utf8').trim(), '700', 'mktemp -d must allocate an owner-only directory');
  assert.throws(() => lstatSync(workspace), { code: 'ENOENT' }, 'the workspace must be removed on exit');
  assert.deepEqual(readdirSync(f.scratch).sort(), ['keep'], 'the supplied TMPDIR keeps only its own entries');
  assert.equal(readFileSync(join(f.decoy, 'decoy-sentinel'), 'utf8'), 'decoy',
    'cleanup must leave the word-split sibling of the spaced TMPDIR untouched');
});

test('cleanup under a failing exit still removes the workspace beneath a spaced TMPDIR', (t) => {
  const f = fixture(t);
  const result = probe(f, `[[ -d "$SECURITY_WS" ]] || exit 71
touch "$SECURITY_WS/cleanup-sentinel"
printf '%s' "$SECURITY_WS" > "$OBSERVED"
exit 42
`);

  assert.equal(result.signal, null);
  assert.equal(result.status, 42, `the probe's own failure status must surface: ${result.stderr}`);
  const workspace = readFileSync(f.observed, 'utf8');
  assert.throws(() => lstatSync(workspace), { code: 'ENOENT' }, 'the workspace must be removed on a failing exit');
  assert.deepEqual(readdirSync(f.scratch).sort(), ['keep'], 'the supplied TMPDIR keeps only its own entries');
  assert.equal(readFileSync(join(f.decoy, 'decoy-sentinel'), 'utf8'), 'decoy');
});

// The quoting regression, reintroduced: the workspace path splits on the
// spaces in TMPDIR, rm removes the first fragment — the decoy — and leaves
// the real workspace behind. This is what the decoy and removal assertions
// above, and the section 2e check inside e2e.sh, exist to detect.
test('an unquoted cleanup removes the word-split sibling instead of the workspace', (t) => {
  const f = fixture(t);
  const result = probe(f, `[[ -d "$SECURITY_WS" ]] || exit 71
printf '%s' "$SECURITY_WS" > "$OBSERVED"
`, { buggy: true });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, `the buggy probe must still run to completion: ${result.stderr}`);
  const workspace = readFileSync(f.observed, 'utf8');
  assert.equal(existsSync(join(f.decoy, 'decoy-sentinel')), false,
    'the word-split sibling is gone — the positive assertion detects this');
  assert.ok(lstatSync(workspace).isDirectory(),
    'the real workspace leaks — the removal assertion detects this');
  rmSync(workspace, { recursive: true, force: true });
});

// The script's own section 2e rehearsal, verbatim. With intact quoting the
// decoy at the root's first word-split fragment must survive; with the
// regression reintroduced the rehearsal must stop the suite instead of
// passing a leaked workspace.
function section2e() {
  const source = readFileSync(script, 'utf8');
  const start = source.indexOf('log "2e.');
  const endMarker = 'ok "SECURITY_WS creation succeeds beneath a TMPDIR containing spaces"';
  const end = source.indexOf(endMarker, start);
  assert.ok(start >= 0 && end > start, 'e2e.sh must contain the section 2e rehearsal');
  return source.slice(start, end + endMarker.length);
}

function rehearse2e(t, { buggy = false } = {}) {
  const root = mkdtempSync(join(tmpdir(), 'locus-secws-2e-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  // A space-free suite home: the first space in SECURITY_WS is then the one in
  // "security  ws temp root", so an unquoted cleanup strikes the decoy first.
  const home = join(root, 'suite-home');
  mkdirSync(home);
  const { cleanup, create } = lifecycle();
  const override = buggy
    ? 'e2e_cleanup() { rm -rf "$LOCUS_HOME" "${WS_DIR:-}" "${GRAPH_HOME:-}" "${SECURITY_HOME:-}" ${SECURITY_WS:-}; }'
    : '';
  const result = spawnSync('bash', ['-c', `set -euo pipefail
${cleanup}
${create}
${override}
log() { :; }
ok() { printf 'ok %s\\n' "$*"; }
die() { printf 'FAIL %s\\n' "$*" >&2; exit 1; }
LOCUS_HOME="$1"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
${section2e()}
`, 'security-ws-2e', home], { cwd: root, encoding: 'utf8' });
  return { result, home, root };
}

test('section 2e plants a word-split decoy that survives the rehearsal', (t) => {
  const { result, home } = rehearse2e(t);
  assert.equal(result.signal, null);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /ok SECURITY_WS creation succeeds beneath a TMPDIR containing spaces/);
  const decoy = join(home, 'security');
  const nested = join(home, 'security  ws temp root');
  assert.ok(existsSync(join(decoy, 'decoy-sentinel')),
    'the decoy at the word-split fragment must survive exit cleanup');
  assert.ok(lstatSync(nested).isDirectory(), 'the supplied temporary root is preserved');
  assert.deepEqual(readdirSync(nested).sort(), ['security-workspace-path'],
    'the rehearsed workspace is gone; only its path record remains');
  assert.throws(
    () => lstatSync(readFileSync(join(nested, 'security-workspace-path'), 'utf8')),
    { code: 'ENOENT' });
});

test('section 2e stops the suite when cleanup splits the spaced workspace path', (t) => {
  const { result, home } = rehearse2e(t, { buggy: true });
  assert.equal(result.signal, null);
  assert.notEqual(result.status, 0, 'the rehearsal must fail once cleanup misbehaves');
  assert.match(result.stderr, /FAIL rehearsed SECURITY_WS survived exit cleanup/);
  const nested = join(home, 'security  ws temp root');
  assert.equal(existsSync(join(home, 'security', 'decoy-sentinel')), false,
    'the unquoted rm struck the word-split sibling');
  const leaked = readFileSync(join(nested, 'security-workspace-path'), 'utf8');
  assert.ok(lstatSync(leaked).isDirectory(), 'the unquoted rm leaked the real workspace');
  rmSync(leaked, { recursive: true, force: true });
});
