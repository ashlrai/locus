// Run: node --test scripts/e2e-home-rehearsal.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { lstatSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const script = join(root, 'scripts', 'e2e.sh');

// The unit under test is the script's own section 2g rehearsal: a subshell
// exports a nested, space-containing TMPDIR, selects it as the temporary root
// for create_locus_home, and relies on its own EXIT trap for cleanup so the
// main suite home survives. The probes below run that block verbatim; stubs
// provide only log/ok/die plus, where noted, a recording mktemp.
function rehearsalParts() {
  const source = readFileSync(script, 'utf8');
  const cleanup = source.match(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m)?.[0];
  const createHome = source.match(/^create_locus_home\(\) \{[\s\S]*?^\}/m)?.[0];
  const start = source.indexOf('log "2g.');
  const endMarker = 'ok "home creation succeeds beneath a TMPDIR containing spaces and is cleaned on exit"';
  const end = source.indexOf(endMarker, start);
  assert.ok(cleanup && createHome && start >= 0 && end > start,
    'e2e.sh must contain the section 2g home-creation rehearsal');
  return { cleanup, createHome, rehearsal: source.slice(start, end + endMarker.length) };
}

function fixture(t) {
  const dir = mkdtempSync(join(tmpdir(), 'locus-e2e-home-rehearsal-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  return dir;
}

// Runs the real section 2g with LOCUS_HOME pointing at a prepared main home.
// The block owns the subshell trap itself, so the main home and its nested
// temporary root stay inspectable after the spawn exits.
function rehearse(home, { shim = '', extraEnv = {} } = {}) {
  const { cleanup, createHome, rehearsal } = rehearsalParts();
  return spawnSync('bash', ['-c', `set -euo pipefail
${cleanup}
${shim}
${createHome}
log() { :; }
ok() { printf 'ok %s\\n' "$*"; }
die() { printf 'FAIL %s\\n' "$*" >&2; exit 1; }
LOCUS_HOME="$1"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
${rehearsal}
`, 'home-2g', home], { env: { ...process.env, ...extraEnv }, encoding: 'utf8' });
}

test('section 2g selects the space-containing TMPDIR and creates the home beneath it', (t) => {
  const dir = fixture(t);
  const home = join(dir, 'main home');
  const outer = join(dir, 'outer tmp');
  const templates = join(dir, 'mktemp-templates');
  const argc = join(dir, 'mktemp-argc');
  mkdirSync(home);
  mkdirSync(outer);
  // Record the requested templates and argument count, then let the real
  // command create the directory it was given. The outer TMPDIR stays visible
  // to prove the subshell-selected root is what reaches mktemp.
  const shim = `mktemp() {
  printf '%s\\n' "$2" >> "$MKTEMP_TEMPLATES"
  printf '%s' "$#" > "$MKTEMP_ARGC"
  command mktemp "$@"
}
`;

  const result = rehearse(home, {
    shim,
    extraEnv: { TMPDIR: outer, MKTEMP_TEMPLATES: templates, MKTEMP_ARGC: argc },
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, `the home rehearsal must succeed: ${result.stderr}`);
  assert.match(result.stdout,
    /ok home creation succeeds beneath a TMPDIR containing spaces and is cleaned on exit/);
  const selected = join(home, 'home temp root');
  assert.equal(readFileSync(argc, 'utf8'), '2',
    'mktemp must receive exactly -d and one template');
  assert.deepEqual(readFileSync(templates, 'utf8').trim().split('\n'),
    [`${selected}/locus-e2e.XXXXXX`],
    'the subshell-selected root must reach mktemp inside one template argument');
  const recorded = readFileSync(join(selected, 'home-path'), 'utf8');
  assert.equal(dirname(recorded), selected,
    'the rehearsal home is created directly beneath the selected TMPDIR');
  assert.match(recorded, /\/locus-e2e\.[^/\s]+$/,
    'the home name is one unsplit component');
  assert.throws(() => lstatSync(recorded), { code: 'ENOENT' },
    'the rehearsal home must be removed on subshell exit');
  assert.deepEqual(readdirSync(selected), ['home-path'],
    'only the recorded home path may remain beneath the selected root');
  assert.deepEqual(readdirSync(outer), [],
    'the outer TMPDIR must not receive the suite home');
  assert.deepEqual(readdirSync(home), ['home temp root'],
    'the subshell trap must leave the main home and the selected root intact');
});

// A mktemp regression that relocates the rehearsal home outside the selected
// root must stop the suite; the subshell trap still removes the misplaced
// directory before the failure reaches the caller.
test('a rehearsal home created outside the selected TMPDIR stops the suite', (t) => {
  const dir = fixture(t);
  const home = join(dir, 'main home');
  const wrongRoot = join(dir, 'wrong root');
  const created = join(dir, 'home-created');
  mkdirSync(home);
  mkdirSync(wrongRoot);
  const shim = `mktemp() {
  local d
  if [[ "\${2:-}" == */locus-e2e.XXXXXX ]]; then
    d="$(command mktemp -d "$WRONG_ROOT/locus-e2e.XXXXXX")"
    printf '%s' "$d" > "$HOME_CREATED"
    printf '%s\\n' "$d"
    return
  fi
  command mktemp "$@"
}
`;

  const result = rehearse(home, {
    shim,
    extraEnv: { WRONG_ROOT: wrongRoot, HOME_CREATED: created },
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 1, 'the containment check must fail the suite');
  assert.match(result.stderr, /FAIL home was not created beneath the space-containing TMPDIR/);
  assert.doesNotMatch(result.stdout, /ok home creation succeeds/);
  const misplaced = readFileSync(created, 'utf8');
  assert.equal(dirname(misplaced), wrongRoot);
  assert.match(misplaced, /\/locus-e2e\.[^/]+$/);
  assert.throws(() => lstatSync(misplaced), { code: 'ENOENT' },
    'the misplaced home must be removed by the subshell exit trap');
  assert.deepEqual(readdirSync(wrongRoot), []);
  assert.deepEqual(readdirSync(join(home, 'home temp root')), [],
    'the rehearsal exits before recording a home path');
});
