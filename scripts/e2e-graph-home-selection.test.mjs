// Run: node --test scripts/e2e-graph-home-selection.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { lstatSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const script = join(root, 'scripts', 'e2e.sh');

// The unit under test is the script's graph home lifecycle: the
// create_graph_home function selects the temporary root (${TMPDIR:-/tmp})
// and creates GRAPH_HOME there, and the e2e_cleanup EXIT trap removes it.
// The probes below run both verbatim; shims provide only a recording mktemp.
// The section-16 containment check on a relocated home is covered by
// scripts/e2e-graph-isolation.test.mjs.
function graphParts() {
  const source = readFileSync(script, 'utf8');
  const cleanup = source.match(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m)?.[0];
  const createHome = source.match(/^create_graph_home\(\) \{[\s\S]*?^\}/m)?.[0];
  assert.ok(cleanup && createHome,
    'e2e.sh must contain the graph home lifecycle');
  return { cleanup, createHome };
}

function fixture(t) {
  const dir = mkdtempSync(join(tmpdir(), 'locus-e2e-graph-home-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  return dir;
}

// Runs create_graph_home under the real EXIT trap. tmpdir === undefined
// leaves TMPDIR unset; extraEnv feeds shim record files. Returns the spawn
// result and the home path the probe writes before exit cleanup runs.
function select(t, { tmpdir, shim = '', tail = '', extraEnv = {} }) {
  const dir = fixture(t);
  const observed = join(dir, 'observed-home');
  const mainHome = join(dir, 'main-home');
  mkdirSync(mainHome);
  const { cleanup, createHome } = graphParts();
  const env = { ...process.env, MAIN_HOME: mainHome, OBSERVED_HOME: observed, ...extraEnv };
  if (tmpdir === undefined) delete env.TMPDIR;
  else env.TMPDIR = tmpdir;
  const result = spawnSync('bash', ['-c', `set -euo pipefail
${cleanup}
${createHome}
${shim}
LOCUS_HOME="$MAIN_HOME"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
trap 'e2e_cleanup' EXIT
create_graph_home
${tail}
printf '%s' "$GRAPH_HOME" > "$OBSERVED_HOME"
`], { env, encoding: 'utf8' });
  return { dir, observed, result };
}

test('GRAPH_HOME is created under the supplied TMPDIR', (t) => {
  const dir = fixture(t);
  const scratch = join(dir, 'custom tmp');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');

  const { observed, result } = select(t, { tmpdir: scratch, tail: `
[[ -d "$GRAPH_HOME" && -w "$GRAPH_HOME" && -x "$GRAPH_HOME" ]] || exit 71
mkdir -p "$GRAPH_HOME/bindings/nested dir" || exit 72
printf 'payload' > "$GRAPH_HOME/bindings/nested dir/f.toml" || exit 73
[[ "$(cat "$GRAPH_HOME/bindings/nested dir/f.toml")" == payload ]] || exit 74
` });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, result.stderr);
  const home = readFileSync(observed, 'utf8');
  assert.equal(dirname(home), scratch, 'the graph home is created directly beneath TMPDIR');
  assert.match(home, /\/locus-graph-imp\.[A-Za-z0-9]{6}$/, 'the mktemp prefix and suffix shape hold');
  assert.throws(() => lstatSync(home), { code: 'ENOENT' },
    'the populated graph home must be removed on exit');
  assert.deepEqual(readdirSync(scratch), ['keep'], 'cleanup must preserve unrelated files');
});

test('a supplied TMPDIR with spaces reaches mktemp inside one template argument', (t) => {
  const dir = fixture(t);
  const scratch = join(dir, 'scratch dir with  spaces');
  const argc = join(dir, 'argc');
  const template = join(dir, 'template');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  // A spaced parent must arrive inside one quoted template; record argv, then
  // let the real command create the directory it was given.
  const shim = `mktemp() {
  printf '%s' "$#" > "$MKTEMP_ARGC"
  printf '%s' "$2" > "$MKTEMP_TEMPLATE"
  command mktemp "$@"
}
`;

  const { observed, result } = select(t, {
    tmpdir: scratch, shim,
    extraEnv: { MKTEMP_ARGC: argc, MKTEMP_TEMPLATE: template },
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(readFileSync(argc, 'utf8'), '2', 'mktemp must receive exactly -d and one template');
  assert.equal(readFileSync(template, 'utf8'), `${scratch}/locus-graph-imp.XXXXXX`,
    'the spaced root must arrive inside a single template argument');
  const home = readFileSync(observed, 'utf8');
  assert.equal(dirname(home), scratch);
  assert.match(home, /\/locus-graph-imp\.[^/\s]+$/, 'the home name is one unsplit component');
  assert.throws(() => lstatSync(home), { code: 'ENOENT' });
  assert.deepEqual(readdirSync(scratch), ['keep']);
});

for (const [label, tmpdir] of [['unset', undefined], ['empty', '']]) {
  test(`the graph home root falls back to /tmp when TMPDIR is ${label}`, (t) => {
    const dir = fixture(t);
    const controlled = join(dir, 'controlled');
    const template = join(dir, 'template');
    mkdirSync(controlled);
    // Record the requested template and redirect creation into the controlled
    // directory so the probe never writes to the real /tmp.
    const shim = `mktemp() {
  if [[ "\${2:-}" == /tmp/locus-graph-imp.XXXXXX ]]; then
    printf '%s' "$2" > "$MKTEMP_TEMPLATE"
    command mktemp -d "$CONTROLLED/locus-graph-imp.XXXXXX"
    return
  fi
  command mktemp "$@"
}
`;

    const { observed, result } = select(t, {
      tmpdir, shim,
      extraEnv: { CONTROLLED: controlled, MKTEMP_TEMPLATE: template },
    });

    assert.equal(result.signal, null);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(readFileSync(template, 'utf8'), '/tmp/locus-graph-imp.XXXXXX',
      `the script must select the /tmp root when TMPDIR is ${label}`);
    assert.equal(dirname(readFileSync(observed, 'utf8')), controlled);
    assert.deepEqual(readdirSync(controlled), [], 'the redirected home must be cleaned up');
  });
}

test('an inherited GRAPH_HOME does not change the selected root', (t) => {
  const dir = fixture(t);
  const scratch = join(dir, 'custom tmp');
  const inherited = join(dir, 'inherited graph home');
  mkdirSync(scratch);
  mkdirSync(inherited);
  writeFileSync(join(inherited, 'keep'), 'inherited');

  const { observed, result } = select(t, {
    tmpdir: scratch, extraEnv: { GRAPH_HOME: inherited },
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, result.stderr);
  const home = readFileSync(observed, 'utf8');
  assert.equal(dirname(home), scratch,
    'the selected root is the supplied TMPDIR, not the inherited home');
  assert.notEqual(home, inherited);
  assert.throws(() => lstatSync(home), { code: 'ENOENT' });
  assert.equal(readFileSync(join(inherited, 'keep'), 'utf8'), 'inherited',
    'the inherited home must survive untouched');
});

// The suite rehearses two unconditional creations; each call must yield a
// distinct directory directly beneath the supplied TMPDIR.
test('repeated graph home creation yields distinct directories under TMPDIR', (t) => {
  const dir = fixture(t);
  const scratch = join(dir, 'custom tmp');
  const first = join(dir, 'first-home');
  const second = join(dir, 'second-home');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');

  const { result } = select(t, {
    tmpdir: scratch,
    extraEnv: { FIRST_HOME: first, SECOND_HOME: second },
    tail: `
graph_home_a="$GRAPH_HOME"
create_graph_home
graph_home_b="$GRAPH_HOME"
[[ "$graph_home_a" != "$graph_home_b" ]] || exit 75
GRAPH_HOME=""
printf '%s' "$graph_home_a" > "$FIRST_HOME"
printf '%s' "$graph_home_b" > "$SECOND_HOME"
rm -rf "$graph_home_a" "$graph_home_b"
`,
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, result.stderr);
  const homeA = readFileSync(first, 'utf8');
  const homeB = readFileSync(second, 'utf8');
  assert.notEqual(homeA, homeB, 'repeated creation must not reuse a directory');
  assert.equal(dirname(homeA), scratch);
  assert.equal(dirname(homeB), scratch);
  assert.match(homeA, /\/locus-graph-imp\.[^/\s]+$/);
  assert.match(homeB, /\/locus-graph-imp\.[^/\s]+$/);
  assert.throws(() => lstatSync(homeA), { code: 'ENOENT' });
  assert.throws(() => lstatSync(homeB), { code: 'ENOENT' });
  assert.deepEqual(readdirSync(scratch), ['keep']);
});
