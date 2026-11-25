// Run: node --test scripts/e2e-temp.test.mjs
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { existsSync, lstatSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const script = join(root, 'scripts', 'e2e.sh');

test('temporary directory reference matches the E2E script', () => {
  const source = readFileSync(script, 'utf8');
  const doc = readFileSync(join(root, 'docs', 'e2e-temp-resource-types.md'), 'utf8');
  const created = [...source.matchAll(/^\s*(\w+)="\$\(mktemp -d "\$\{TMPDIR:-\/tmp\}\/([^/"\s]+)XXXXXX"\)"/gm)]
    .map(([, variable, prefix]) => [variable, prefix]);
  const documented = [...doc.matchAll(/^\| `(\w+)` \| `([^`]+)` \| ([^|\n]+) \|$/gm)]
    .map(([, variable, prefix, purpose]) => {
      assert.ok(purpose.trim(), `${variable} needs a purpose`);
      return [variable, prefix];
    });

  assert.equal(created.length, 5, 'expected five temporary directory assignments');
  assert.equal([...source.matchAll(/\bmktemp\s+-d\b/g)].length, created.length,
    'every mktemp directory must be represented in the inventory');
  assert.equal(new Set(created.map(([variable]) => variable)).size, created.length);
  assert.deepEqual(documented.sort(), created.sort(), 'documented variables and prefixes must match the script');
});

test('security directory guide maps the script fixtures and their cleanup', () => {
  const source = readFileSync(script, 'utf8');
  const doc = readFileSync(join(root, 'docs', 'e2e-security-directories.md'), 'utf8');
  const rows = [...doc.matchAll(/^\| `(SECURITY_(?:HOME|WS))` \| `([^`]+)` \| ([^|\n]+) \|$/gm)];
  assert.deepEqual(rows.map(([, variable]) => variable), ['SECURITY_HOME', 'SECURITY_WS']);
  for (const [, variable, prefix, purpose] of rows) {
    assert.ok(purpose.trim(), `${variable} needs a purpose`);
    assert.ok(source.includes(`${variable}="$(mktemp -d "\${TMPDIR:-/tmp}/${prefix}XXXXXX")"`),
      `${variable} prefix must match its creation in the script`);
    assert.ok(source.includes(`"\${${variable}:-}"`), `${variable} must be covered by cleanup`);
  }
  assert.match(doc, /`LOCUS_HOME`/);
  assert.match(doc, /`\.locus\.toml` symlink/);
  assert.match(doc, /`EXIT` trap/);
});

function setup(t) {
  const root = mkdtempSync(join(tmpdir(), 'locus-e2e-temp-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const bin = join(root, 'bin');
  mkdirSync(bin);
  // Avoid building binaries and fail immediately after the cleanup trap is
  // set. Sections 2d and 2f rehearse `init` inside their subshells and 2f
  // requires the home to be populated, so the stub's init writes a stamp the
  // way the real command populates the home; `workspace` writes the allowlist.
  // Every other command fails.
  writeFileSync(join(bin, 'cargo'), '#!/bin/sh\nexit 0\n', { mode: 0o755 });
  const locus = join(bin, 'locus');
  writeFileSync(locus, `#!/bin/sh
case "$1" in
  init) printf 'stub init\\n' > "$LOCUS_HOME/init-stamp" && exit 0 ;;
  workspace) printf '# stub workspace config\\n' > .locus.toml && exit 0 ;;
esac
exit 1
`, { mode: 0o755 });
  return { root, bin, locus };
}

function run({ bin, locus }, temp, extraEnv = {}) {
  return spawnSync('bash', [script], {
    env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, TMPDIR: temp,
      LOCUS_BIN: locus, MCP_BIN: locus, ...extraEnv },
    encoding: 'utf8',
  });
}

test('successful exit removes the populated isolated home', (t) => {
  const fixture = setup(t);
  const scratch = join(fixture.root, 'custom tmp');
  const observed = join(fixture.root, 'observed-home');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');

  // Execute the script's actual home setup and EXIT trap in a successful shell.
  // The full e2e success path is covered separately by e2e.test.mjs.
  const source = readFileSync(script, 'utf8');
  const start = source.indexOf('create_locus_home() {\n');
  const trap = '\ne2e_install_traps\n';
  const end = source.indexOf(trap, start);
  assert.ok(start >= 0 && end > start, 'home setup and cleanup trap must exist');
  const setupSource = source.slice(start, end + trap.length);
  const result = spawnSync('bash', ['-c', `set -euo pipefail
log() { :; }
${setupSource}
[[ -d "$LOCUS_HOME" ]]
touch "$LOCUS_HOME/cleanup-sentinel"
printf '%s' "$LOCUS_HOME" > "$OBSERVED_HOME"
`], {
    env: { ...process.env, TMPDIR: scratch, OBSERVED_HOME: observed },
    encoding: 'utf8',
  });

  assert.equal(result.status, 0, 'the shell must exit successfully');
  assert.equal(result.signal, null);
  const home = readFileSync(observed, 'utf8');
  assert.equal(dirname(home), scratch);
  assert.match(home, /\/locus-e2e\.[^/]+$/);
  assert.throws(() => lstatSync(home), { code: 'ENOENT' }, 'the home must be removed');
  assert.deepEqual(readdirSync(scratch), ['keep']);
});

test('successful home cleanup preserves an inherited home and symlink target', (t) => {
  const fixture = setup(t);
  const scratch = join(fixture.root, 'tmp $literal; [test]');
  const inherited = join(fixture.root, 'inherited home');
  const target = join(fixture.root, 'external target');
  const observed = join(fixture.root, 'observed-home');
  mkdirSync(scratch);
  mkdirSync(inherited);
  mkdirSync(target);
  writeFileSync(join(inherited, 'keep'), 'inherited');
  writeFileSync(join(target, 'keep'), 'external');
  symlinkSync(target, join(scratch, 'target-link'));

  const source = readFileSync(script, 'utf8');
  const start = source.indexOf('create_locus_home() {\n');
  const trap = '\ne2e_install_traps\n';
  const end = source.indexOf(trap, start);
  assert.ok(start >= 0 && end > start, 'home setup and cleanup trap must exist');
  const result = spawnSync('bash', ['-c', `set -euo pipefail
log() { :; }
${source.slice(start, end + trap.length)}
[[ -d "$LOCUS_HOME" && "$LOCUS_HOME" != "$INHERITED_HOME" ]]
mkdir "$LOCUS_HOME/nested"
touch "$LOCUS_HOME/nested/sentinel"
ln -s "$TMPDIR/target-link" "$LOCUS_HOME/nested/external-link"
[[ -f "$LOCUS_HOME/nested/external-link/keep" ]]
printf '%s' "$LOCUS_HOME" > "$OBSERVED_HOME"
`], {
    env: { ...process.env, TMPDIR: scratch, LOCUS_HOME: inherited,
      INHERITED_HOME: inherited, OBSERVED_HOME: observed },
    encoding: 'utf8',
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, 'the shell must exit successfully');
  const home = readFileSync(observed, 'utf8');
  assert.equal(dirname(home), scratch);
  assert.throws(() => lstatSync(home), { code: 'ENOENT' }, 'the temporary home must be removed');
  assert.equal(readFileSync(join(inherited, 'keep'), 'utf8'), 'inherited');
  assert.equal(readFileSync(join(target, 'keep'), 'utf8'), 'external');
  assert.deepEqual(readdirSync(scratch), ['target-link']);
});

test('uses TMPDIR and cleans the isolated home after a later failure', (t) => {
  const fixture = setup(t);
  const scratch = join(fixture.root, 'custom tmp');
  const homeCreated = join(fixture.root, 'home-created');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  const realMktemp = execFileSync('which', ['mktemp'], { encoding: 'utf8' }).trim();
  writeFileSync(join(fixture.bin, 'mktemp'), `#!/bin/sh
case "$2" in
  */locus-e2e.XXXXXX)
    home="$("$REAL_MKTEMP" "$@")" || exit 1
    test -d "$home" || exit 1
    # The section 2g rehearsal selects a nested TMPDIR and calls mktemp again;
    # record only the suite home, the first locus-e2e directory this run makes.
    if [ ! -f "$HOME_CREATED" ]; then
      printf '%s' "$home" > "$HOME_CREATED"
      touch "$home/sentinel"
    fi
    printf '%s\n' "$home"
    ;;
  *) exec "$REAL_MKTEMP" "$@" ;;
esac
`, { mode: 0o755 });
  const result = run(fixture, scratch, { HOME_CREATED: homeCreated, REAL_MKTEMP: realMktemp });

  assert.notEqual(result.status, 0);
  assert.equal(result.signal, null);
  assert.match(result.stderr, /fresh quickstart failed/);
  const home = readFileSync(homeCreated, 'utf8');
  assert.equal(dirname(home), scratch);
  assert.match(home, /\/locus-e2e\.[^/]+$/);
  assert.equal(existsSync(home), false, 'home and its sentinel must be removed on exit');
  assert.deepEqual(readdirSync(scratch), ['keep']);
});

test('a home created outside TMPDIR fails the temporary-root check', (t) => {
  const fixture = setup(t);
  const scratch = join(fixture.root, 'custom tmp');
  const wrongRoot = join(fixture.root, 'wrong root');
  mkdirSync(scratch);
  mkdirSync(wrongRoot);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  const realMktemp = execFileSync('which', ['mktemp'], { encoding: 'utf8' }).trim();
  // Answer with the expected name shape but relocate the created home outside
  // TMPDIR; the in-script check must stop the suite before the workload runs.
  writeFileSync(join(fixture.bin, 'mktemp'), `#!/bin/sh
case "$2" in
  "$TMPDIR"/locus-e2e.XXXXXX)
    home="$("$REAL_MKTEMP" -d "$WRONG_ROOT/locus-e2e.XXXXXX")" || exit $?
    touch "$home/sentinel" || exit 1
    printf '%s\n' "$home"
    ;;
  *) exec "$REAL_MKTEMP" "$@" ;;
esac
`, { mode: 0o755 });

  const result = run(fixture, scratch, { REAL_MKTEMP: realMktemp, WRONG_ROOT: wrongRoot });

  assert.equal(result.status, 1);
  assert.equal(result.signal, null);
  assert.match(result.stderr, /LOCUS_HOME was not created directly beneath TMPDIR/);
  assert.doesNotMatch(result.stdout, /ok  isolated home is created directly beneath TMPDIR/);
  assert.deepEqual(readdirSync(wrongRoot), [], 'the misplaced home must be removed on exit');
  assert.deepEqual(readdirSync(scratch), ['keep']);
});

test('isolated home is created directly beneath a TMPDIR containing spaces', (t) => {
  const fixture = setup(t);
  const scratch = join(fixture.root, 'scratch dir with  spaces');
  const observed = join(fixture.root, 'observed-home');
  const mktempArgs = join(fixture.root, 'mktemp-args');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  // A spaced parent must reach mktemp inside one quoted template. Record the
  // arguments, then let the real command create the directory it was given.
  const realMktemp = execFileSync('which', ['mktemp'], { encoding: 'utf8' }).trim();
  writeFileSync(join(fixture.bin, 'mktemp'), `#!/bin/sh
printf '%s\\n' "$@" > "$MKTEMP_ARGS"
printf '%s' "$#" >> "$MKTEMP_ARGS"
exec "$REAL_MKTEMP" "$@"
`, { mode: 0o755 });
  // Exercise the script's own home creation, parent check, and EXIT cleanup.
  const source = readFileSync(script, 'utf8');
  const start = source.indexOf('create_locus_home() {\n');
  const endMarker = 'ok "isolated home is created directly beneath TMPDIR"\nfi';
  const end = source.indexOf(endMarker, start);
  assert.ok(start >= 0 && end > start, 'home setup and parent check must exist');
  const result = spawnSync('bash', ['-c', `set -euo pipefail
log() { :; }
ok() { printf 'ok %s\\n' "$*"; }
die() { printf 'FAIL %s\\n' "$*" >&2; exit 1; }
${source.slice(start, end + endMarker.length)}
touch "$LOCUS_HOME/cleanup-sentinel"
printf '%s' "$LOCUS_HOME" > "$OBSERVED_HOME"
`], {
    env: { ...process.env, PATH: `${fixture.bin}:${process.env.PATH}`, TMPDIR: scratch,
      OBSERVED_HOME: observed, MKTEMP_ARGS: mktempArgs, REAL_MKTEMP: realMktemp },
    encoding: 'utf8',
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /ok isolated home is created directly beneath TMPDIR/,
    'the script must confirm the home lands beneath the spaced parent');
  const [flag, template, argc] = readFileSync(mktempArgs, 'utf8').split('\n');
  assert.equal(flag, '-d');
  assert.equal(template, `${scratch}/locus-e2e.XXXXXX`,
    'the spaced parent must arrive inside one template argument');
  assert.equal(argc, '2', 'mktemp must receive exactly -d and one template');
  const home = readFileSync(observed, 'utf8');
  assert.equal(dirname(home), scratch, 'created directly beneath the spaced TMPDIR');
  assert.match(home, /\/locus-e2e\.[^/\s]+$/, 'home name is one unsplit component');
  assert.throws(() => lstatSync(home), { code: 'ENOENT' }, 'home must be removed on exit');
  assert.deepEqual(readdirSync(scratch), ['keep'], 'cleanup must preserve unrelated files');
});

test('home under a spaced TMPDIR is usable and its real path stays beneath the parent', (t) => {
  const fixture = setup(t);
  const scratch = join(fixture.root, 'usable tmp dir');
  const observed = join(fixture.root, 'observed-home');
  mkdirSync(scratch);
  const source = readFileSync(script, 'utf8');
  const start = source.indexOf('create_locus_home() {\n');
  const endMarker = 'ok "isolated home is created directly beneath TMPDIR"\nfi';
  const end = source.indexOf(endMarker, start);
  assert.ok(start >= 0 && end > start, 'home setup and parent check must exist');
  // Use the home the way the suite does: nested spaced paths, write, read back,
  // and a physical-path containment check that a plain string compare cannot give.
  const result = spawnSync('bash', ['-c', `set -euo pipefail
log() { :; }
ok() { :; }
die() { printf 'FAIL %s\\n' "$*" >&2; exit 1; }
${source.slice(start, end + endMarker.length)}
[[ -w "$LOCUS_HOME" && -x "$LOCUS_HOME" ]]
mkdir -p "$LOCUS_HOME/bindings/with space"
printf 'payload' > "$LOCUS_HOME/bindings/with space/f.toml"
[[ "$(cat "$LOCUS_HOME/bindings/with space/f.toml")" == payload ]]
real_parent="$(cd "$TMPDIR" && pwd -P)"
real_home="$(cd "$LOCUS_HOME" && pwd -P)"
[[ "$real_home" == "$real_parent"/* && "$real_home" != *..* ]]
printf '%s' "$LOCUS_HOME" > "$OBSERVED_HOME"
`], {
    env: { ...process.env, TMPDIR: scratch, OBSERVED_HOME: observed },
    encoding: 'utf8',
  });

  assert.equal(result.status, 0, result.stderr);
  assert.equal(dirname(readFileSync(observed, 'utf8')), scratch);
  assert.deepEqual(readdirSync(scratch), [], 'usable home must still be cleaned on exit');
});

test('an unhandled setup command failure removes the populated LOCUS_HOME', (t) => {
  const fixture = setup(t);
  const scratch = join(fixture.root, 'custom tmp');
  const inherited = join(fixture.root, 'inherited home');
  const observed = join(fixture.root, 'home-before-failure');
  mkdirSync(scratch);
  mkdirSync(inherited);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  writeFileSync(join(inherited, 'keep'), 'inherited');
  // Fail the first setup command after the real EXIT trap is installed.
  writeFileSync(join(fixture.bin, 'mkdir'), `#!/bin/sh
if [ "$1" = -p ] && [ "$2" = "$LOCUS_HOME/custom temp" ]; then
  test -d "$LOCUS_HOME" || exit 90
  touch "$LOCUS_HOME/sentinel" || exit 91
  printf '%s' "$LOCUS_HOME" > "$OBSERVED_HOME"
  exit 73
fi
exec /bin/mkdir "$@"
`, { mode: 0o755 });

  const result = run(fixture, scratch, { LOCUS_HOME: inherited, OBSERVED_HOME: observed });

  assert.equal(result.signal, null);
  assert.equal(result.status, 73, 'the failing command status must survive cleanup');
  const home = readFileSync(observed, 'utf8');
  assert.equal(dirname(home), scratch);
  assert.match(home, /\/locus-e2e\.[^/]+$/);
  assert.equal(existsSync(home), false, 'the populated home must be removed on failure');
  assert.deepEqual(readdirSync(scratch), ['keep']);
  assert.equal(readFileSync(join(inherited, 'keep'), 'utf8'), 'inherited');
});

test('exports a custom TMPDIR home to children and preserves an inherited home', (t) => {
  const fixture = setup(t);
  const scratch = join(fixture.root, 'custom tmp $literal; [test]');
  const inherited = join(fixture.root, 'existing home');
  const observed = join(fixture.root, 'observed-home');
  mkdirSync(scratch);
  mkdirSync(inherited);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  writeFileSync(join(inherited, 'keep'), 'inherited');
  // The first mkdir after home creation inherits the script's exported home.
  // Record it while it exists, then let the real command proceed normally.
  writeFileSync(join(fixture.bin, 'mkdir'), `#!/bin/sh
case "$LOCUS_HOME" in
  "$TMPDIR"/locus-e2e.*)
    test -d "$LOCUS_HOME" || exit 1
    printf '%s' "$LOCUS_HOME" > "$OBSERVED_HOME"
    touch "$LOCUS_HOME/cleanup-sentinel"
    ;;
esac
exec /bin/mkdir "$@"
`, { mode: 0o755 });

  const result = run(fixture, scratch, { LOCUS_HOME: inherited, OBSERVED_HOME: observed });

  assert.equal(result.status, 1);
  assert.equal(result.signal, null);
  assert.match(result.stderr, /fresh quickstart failed/);
  const home = readFileSync(observed, 'utf8');
  assert.equal(dirname(home), scratch);
  assert.match(home, /\/locus-e2e\.[^/]+$/);
  assert.equal(existsSync(home), false, 'exported home and its sentinel must be removed');
  assert.deepEqual(readdirSync(scratch), ['keep']);
  assert.equal(readFileSync(join(inherited, 'keep'), 'utf8'), 'inherited');
});

test('exit cleanup preserves an inherited graph home before the fallback runs', (t) => {
  const fixture = setup(t);
  const scratch = join(fixture.root, 'scratch');
  const inherited = join(fixture.root, 'unrelated graph home');
  mkdirSync(scratch);
  mkdirSync(inherited);
  writeFileSync(join(inherited, 'keep'), 'unrelated');

  const result = run(fixture, scratch, { GRAPH_HOME: inherited });

  assert.equal(result.status, 1);
  assert.match(result.stderr, /fresh quickstart failed/);
  assert.deepEqual(readdirSync(inherited), ['keep']);
  assert.deepEqual(readdirSync(scratch), []);
});

test('exit cleanup removes the populated graph home after success', (t) => {
  const fixture = setup(t);
  const scratch = join(fixture.root, 'custom tmp $literal; [test]');
  const observed = join(fixture.root, 'graph-home-observed');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  const source = readFileSync(script, 'utf8');
  const cleanup = source.match(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m)?.[0];
  const createGraphHome = source.match(/^create_graph_home\(\) \{[\s\S]*?^\}/m)?.[0];
  assert.ok(cleanup, 'missing e2e cleanup function');
  assert.ok(createGraphHome, 'missing graph home creation');

  const result = spawnSync('bash', ['-c', `set -euo pipefail
${cleanup}
die() { printf 'FAIL %s\n' "$*" >&2; exit 1; }
${createGraphHome}
LOCUS_HOME="$1/main-home"
mkdir "$LOCUS_HOME"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
trap 'e2e_cleanup' EXIT
create_graph_home
[[ -d "$GRAPH_HOME" && "\${GRAPH_HOME%/*}" == "$TMPDIR" && "\${GRAPH_HOME##*/}" == locus-graph-imp.* ]]
touch "$GRAPH_HOME/import-sentinel"
[[ -f "$GRAPH_HOME/import-sentinel" ]]
printf '%s' "$GRAPH_HOME" > "$2"
`, 'graph-cleanup', scratch, observed], {
    env: { ...process.env, TMPDIR: scratch },
    encoding: 'utf8',
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, 'graph cleanup probe must exit successfully');
  const graphHome = readFileSync(observed, 'utf8');
  assert.equal(dirname(graphHome), scratch);
  assert.match(graphHome, /\/locus-graph-imp\.[^/]+$/);
  assert.throws(() => lstatSync(graphHome), { code: 'ENOENT' }, 'graph home must be removed on exit');
  assert.deepEqual(readdirSync(scratch), ['keep'], 'cleanup must preserve unrelated files');
});

test('exit cleanup preserves an inherited workspace before the workspace is created', (t) => {
  const fixture = setup(t);
  const scratch = join(fixture.root, 'scratch');
  const inherited = join(fixture.root, 'unrelated workspace');
  mkdirSync(scratch);
  mkdirSync(inherited);
  writeFileSync(join(inherited, 'keep'), 'unrelated');

  const result = run(fixture, scratch, { WS_DIR: inherited });

  assert.equal(result.status, 1);
  assert.equal(result.signal, null);
  assert.match(result.stderr, /fresh quickstart failed/);
  assert.deepEqual(readdirSync(inherited), ['keep']);
  assert.deepEqual(readdirSync(scratch), []);
});

test('exit cleanup preserves inherited security directories before the probes run', (t) => {
  const fixture = setup(t);
  const scratch = join(fixture.root, 'scratch');
  const inheritedHome = join(fixture.root, 'unrelated security home');
  const inheritedWorkspace = join(fixture.root, 'unrelated security workspace');
  mkdirSync(scratch);
  for (const dir of [inheritedHome, inheritedWorkspace]) {
    mkdirSync(dir);
    writeFileSync(join(dir, 'keep'), 'unrelated');
  }

  const result = run(fixture, scratch, { SECURITY_HOME: inheritedHome, SECURITY_WS: inheritedWorkspace });

  assert.equal(result.status, 1);
  assert.match(result.stderr, /fresh quickstart failed/);
  for (const dir of [inheritedHome, inheritedWorkspace]) assert.deepEqual(readdirSync(dir), ['keep']);
  assert.deepEqual(readdirSync(scratch), []);
});

for (const [label, temp] of [['unset', undefined], ['empty', '']]) {
  test(`${label} TMPDIR falls back to /tmp`, (t) => {
    const fixture = setup(t);
    const scratch = join(fixture.root, 'scratch');
    const capture = join(fixture.root, 'template');
    mkdirSync(scratch);
    const realMktemp = execFileSync('which', ['mktemp'], { encoding: 'utf8' }).trim();
    // Redirect only the main home, requested at the /tmp fallback root; the
    // section 2g rehearsal and the security probes use their own TMPDIR.
    writeFileSync(join(fixture.bin, 'mktemp'), `#!/bin/sh
case "$2" in
  "/tmp/locus-e2e.XXXXXX")
    printf '%s' "$2" > "$PROBE_CAPTURE"
    exec "$REAL_MKTEMP" -d "$PROBE_SCRATCH/locus-e2e.XXXXXX"
    ;;
esac
exec "$REAL_MKTEMP" "$@"
`, { mode: 0o755 });
    const result = run(fixture, temp, {
      REAL_MKTEMP: realMktemp, PROBE_CAPTURE: capture, PROBE_SCRATCH: scratch,
    });

    assert.notEqual(result.status, 0);
    assert.equal(result.signal, null);
    assert.match(result.stderr, /fresh quickstart failed/);
    assert.equal(readFileSync(capture, 'utf8'), '/tmp/locus-e2e.XXXXXX');
    assert.deepEqual(readdirSync(scratch), []);
  });
}

test('a nonexistent TMPDIR fails before creating an isolated home', (t) => {
  const fixture = setup(t);
  const result = run(fixture, join(fixture.root, 'missing'));

  assert.notEqual(result.status, 0);
  assert.equal(result.signal, null);
  assert.match(result.stderr, /mktemp/);
  assert.deepEqual(readdirSync(fixture.root), ['bin']);
});

test('an initial mktemp failure stops before the e2e workload', (t) => {
  const fixture = setup(t);
  const scratch = join(fixture.root, 'custom tmp');
  const inherited = join(fixture.root, 'inherited home');
  const home = join(fixture.root, 'home');
  const attempts = join(fixture.root, 'mktemp-attempts');
  const workload = join(fixture.root, 'workload-started');
  mkdirSync(scratch);
  mkdirSync(inherited);
  mkdirSync(home);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  writeFileSync(join(inherited, 'keep'), 'inherited');
  writeFileSync(join(fixture.bin, 'mktemp'), `#!/bin/sh
printf 'attempt\\n' >> "$MKTEMP_ATTEMPTS"
test "$#" -eq 2 && test "$1" = -d && test "$2" = "$TMPDIR/locus-e2e.XXXXXX" || exit 92
printf 'injected mktemp failure\\n' >&2
exit 73
`, { mode: 0o755 });
  // The first fixture setup invokes mkdir; both binaries are workload entry points.
  const workloadStub = '#!/bin/sh\nprintf started > "$WORKLOAD_MARKER"\nexit 91\n';
  writeFileSync(join(fixture.bin, 'mkdir'), workloadStub, { mode: 0o755 });
  writeFileSync(fixture.locus, workloadStub, { mode: 0o755 });

  const result = run(fixture, scratch, {
    HOME: home, LOCUS_HOME: inherited, LOCUS_CONTROL_CAPABILITY: 'test-capability',
    MKTEMP_ATTEMPTS: attempts, WORKLOAD_MARKER: workload,
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 73, 'the initial mktemp failure must stop the script');
  assert.equal(readFileSync(attempts, 'utf8'), 'attempt\n', 'only the initial mktemp should run');
  assert.match(result.stderr, /injected mktemp failure/);
  assert.match(result.stdout, /ok  locus \+ locus-mcp release binaries/);
  assert.doesNotMatch(result.stdout, /==> 2\./, 'fixture setup must not start');
  assert.equal(existsSync(workload), false, 'the e2e workload must not run');
  assert.deepEqual(readdirSync(scratch), ['keep']);
  assert.equal(readFileSync(join(inherited, 'keep'), 'utf8'), 'inherited');
});

test('workspace allocation failure stops before workspace initialization', (t) => {
  const fixture = setup(t);
  const scratch = join(fixture.root, 'custom tmp');
  const marker = join(fixture.root, 'workspace-initialized');
  const attempts = join(fixture.root, 'workspace-attempts');
  const realMktemp = execFileSync('which', ['mktemp'], { encoding: 'utf8' }).trim();
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  writeFileSync(join(fixture.bin, 'mktemp'), `#!/bin/sh
case "$2" in
  */locus-ws.XXXXXX)
    printf 'attempt\\n' >> "$WORKSPACE_ATTEMPTS"
    printf 'injected workspace allocation failure\\n' >&2
    exit 73 ;;
esac
exec "$REAL_MKTEMP" "$@"
`, { mode: 0o755 });
  writeFileSync(fixture.locus, `#!/bin/sh
case "$1" in
  init) printf 'stub init\\n' > "$LOCUS_HOME/init-stamp"; exit 0 ;;
  workspace) printf 'started\\n' > "$WORKSPACE_MARKER"; exit 0 ;;
esac
exit 1
`, { mode: 0o755 });

  const result = run(fixture, scratch, {
    REAL_MKTEMP: realMktemp, WORKSPACE_ATTEMPTS: attempts, WORKSPACE_MARKER: marker,
  });

  assert.equal(result.signal, null);
  assert.notEqual(result.status, 0, 'allocation failure must stop the script');
  assert.match(result.stderr, /injected workspace allocation failure/);
  assert.match(result.stderr, /workspace was not created beneath the space-containing TMPDIR/);
  assert.equal(readFileSync(attempts, 'utf8'), 'attempt\n');
  assert.equal(existsSync(marker), false, 'workspace initialization must not run');
  assert.deepEqual(readdirSync(scratch), ['keep'], 'cleanup must preserve unrelated files');
});

test('workspace cleanup survives a command failure after leaving the workspace', (t) => {
  const fixture = setup(t);
  const scratch = join(fixture.root, 'custom tmp $literal; [test]');
  const record = join(fixture.root, 'workspace-path');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  const source = readFileSync(script, 'utf8');
  const cleanup = source.match(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m)?.[0];
  const createWorkspace = source.match(/^create_workspace\(\) \{[\s\S]*?^\}/m)?.[0];
  const start = source.indexOf('create_workspace\n# Same containment rule');
  const end = source.indexOf('\n# leave force-pin;', start);
  assert.ok(cleanup && createWorkspace && start >= 0 && end > start, 'missing workspace lifecycle');
  const workspaceSection = source.slice(start, end);
  const probe = `set -euo pipefail
${cleanup}
${createWorkspace}
TMPDIR="$1" LOCUS_HOME="$1/main-home"
GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
mkdir "$LOCUS_HOME"
trap 'e2e_cleanup' EXIT
locus() {
  case "$*" in
    workspace*) touch .locus.toml cleanup-sentinel; printf '%s' "$PWD" > "$WORKSPACE_RECORD" ;;
    'pin personal') printf 'not allowed\\n' >&2; return 1 ;;
    'pin personal --force') return 0 ;;
    *) return 74 ;;
  esac
}
log() { :; }; ok() { :; }; die() { exit 75; }
${workspaceSection}
[[ -f "$WS_DIR/.locus.toml" && -f "$WS_DIR/cleanup-sentinel" && "$PWD" != "$WS_DIR" ]] || exit 74
bash -c 'exit 73'
`;
  const result = spawnSync('bash', ['-c', probe, 'workspace-failure', scratch], {
    env: { ...process.env, WORKSPACE_RECORD: record }, encoding: 'utf8',
  });
  assert.equal(result.signal, null);
  assert.equal(result.status, 73, 'the injected failure must propagate');
  const workspace = readFileSync(record, 'utf8');
  assert.equal(dirname(workspace), scratch);
  assert.match(workspace, /\/locus-ws\.[^/]+$/);
  assert.throws(() => lstatSync(workspace), { code: 'ENOENT' });
  assert.deepEqual(readdirSync(scratch), ['keep']);
});

for (const exitStatus of [0, 73]) {
  test(`workspace honors TMPDIR and is removed on exit ${exitStatus}`, (t) => {
    const fixture = setup(t);
    const scratch = join(fixture.root, 'custom tmp');
    const record = join(fixture.root, 'workspace-path');
    mkdirSync(scratch);
    writeFileSync(join(scratch, 'keep'), 'unrelated');
    const sibling = join(scratch, 'locus-ws.preexisting');
    mkdirSync(join(sibling, 'nested'), { recursive: true });
    writeFileSync(join(sibling, 'nested', 'keep'), 'sibling contents\n');
    const source = readFileSync(script, 'utf8');
    const cleanup = source.match(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m)?.[0];
    const createWorkspace = source.match(/^\s*WS_DIR="\$\(mktemp -d .*\)"$/m)?.[0];
    assert.ok(cleanup, 'missing e2e cleanup function');
    assert.ok(createWorkspace, 'missing workspace creation');
    const probe = `${cleanup}
TMPDIR="$1"
LOCUS_HOME="$TMPDIR/main-home"
GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
mkdir "$LOCUS_HOME"
trap 'e2e_cleanup' EXIT
${createWorkspace}
[[ -d "$WS_DIR" && "\${WS_DIR%/*}" == "$TMPDIR" && "\${WS_DIR##*/}" == locus-ws.* ]] || exit 1
printf '%s' "$WS_DIR" > "$2"
touch "$WS_DIR/cleanup-sentinel"
exit "$3"
`;
    const result = spawnSync('bash', ['-c', probe, 'workspace-cleanup', scratch, record, String(exitStatus)],
      { encoding: 'utf8' });
    assert.equal(result.status, exitStatus, 'workspace creation or cleanup probe failed');
    assert.equal(result.signal, null);
    const workspace = readFileSync(record, 'utf8');
    assert.equal(dirname(workspace), scratch);
    assert.match(workspace, /\/locus-ws\.[^/]+$/);
    assert.throws(() => lstatSync(workspace), { code: 'ENOENT' }, 'workspace must be removed');
    assert.ok(lstatSync(sibling).isDirectory(), 'preexisting sibling must survive cleanup');
    assert.equal(readFileSync(join(sibling, 'nested', 'keep'), 'utf8'), 'sibling contents\n');
    assert.deepEqual(readdirSync(scratch).sort(), ['keep', 'locus-ws.preexisting']);
  });
}

// Runs the real workspace section (not a copy of its mktemp line) with a
// stubbed mktemp that records its template before delegating to the real
// command. With TMPDIR set, the selected root must be that directory and the
// created workspace must sit directly beneath it.
test('e2e.sh workspace section creates WS_DIR directly beneath the supplied TMPDIR', (t) => {
  const fixture = setup(t);
  const scratch = join(fixture.root, 'temp with spaces');
  const templates = join(fixture.root, 'templates');
  const wsUsed = join(fixture.root, 'ws-used');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  const source = readFileSync(script, 'utf8');
  const cleanup = source.match(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m)?.[0];
  const createWorkspace = source.match(/^create_workspace\(\) \{[\s\S]*?^\}/m)?.[0];
  const start = source.indexOf('create_workspace\n# Same containment rule');
  const end = source.indexOf('\n# leave force-pin;', start);
  assert.ok(cleanup && createWorkspace && start >= 0 && end > start, 'missing workspace lifecycle');
  const workspaceSection = source.slice(start, end);
  const probe = `set -euo pipefail
${cleanup}
${createWorkspace}
TMPDIR="$1"
LOCUS_HOME="$TMPDIR/main-home"
GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
mkdir "$LOCUS_HOME"
trap 'e2e_cleanup' EXIT
mktemp() { printf '%s\\n' "$2" >> "$TEST_TEMPLATES"; command mktemp "$@"; }
locus() {
  case "$*" in
    workspace*) touch .locus.toml; printf '%s' "$PWD" > "$WS_USED" ;;
    'pin personal') printf 'not allowed\\n' >&2; return 1 ;;
    'pin personal --force') return 0 ;;
    *) return 74 ;;
  esac
}
log() { :; }; ok() { printf '%s\\n' "$*"; }; die() { printf '%s\\n' "$*" >&2; exit 74; }
${workspaceSection}
`;
  const result = spawnSync('bash', ['-c', probe, 'workspace-root', scratch], {
    env: { ...process.env, TEST_TEMPLATES: templates, WS_USED: wsUsed },
    encoding: 'utf8',
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, `the workspace section must succeed: ${result.stderr}`);
  assert.match(result.stdout, /workspace is created directly beneath TMPDIR/);
  assert.equal(readFileSync(templates, 'utf8').trim(), `${scratch}/locus-ws.XXXXXX`,
    'the section must ask mktemp for a directory rooted at TMPDIR');
  const workspace = readFileSync(wsUsed, 'utf8');
  assert.equal(dirname(workspace), scratch, 'the workspace is created directly beneath TMPDIR');
  assert.match(workspace, /\/locus-ws\.[^/]+$/);
  assert.throws(() => lstatSync(workspace), { code: 'ENOENT' }, 'workspace must be removed on exit');
  assert.deepEqual(readdirSync(scratch), ['keep'], 'cleanup must preserve unrelated files');
});

for (const setting of ['isolated', 'unset', 'empty']) {
  test(`WS_DIR selects the temporary root when TMPDIR is ${setting}`, (t) => {
    const fixture = setup(t);
    const capture = join(fixture.root, 'template');
    const observed = join(fixture.root, 'workspace');
    const source = readFileSync(script, 'utf8');
    const createWorkspace = source.match(/^create_workspace\(\) \{[\s\S]*?^\}/m)?.[0];
    assert.ok(createWorkspace, 'missing workspace creation function');
    const env = { ...process.env, CAPTURE: capture, OBSERVED: observed, CONTROLLED_ROOT: fixture.root };
    if (setting === 'unset') delete env.TMPDIR;
    else env.TMPDIR = setting === 'empty' ? '' : fixture.root;
    const result = spawnSync('bash', ['-c', `set -euo pipefail
${createWorkspace}
mktemp() {
  [[ "$#" == 2 && "$1" == -d ]] || exit 71
  printf '%s' "$2" > "$CAPTURE"
  if [[ -n "\${TMPDIR:-}" ]]; then
    command mktemp "$@"
  else
    command mktemp -d "$CONTROLLED_ROOT/locus-ws.XXXXXX"
  fi
}
create_workspace
[[ -d "$WS_DIR" ]] || exit 72
printf '%s' "$WS_DIR" > "$OBSERVED"
rm -rf -- "$WS_DIR"
`], { env, encoding: 'utf8' });
    assert.equal(result.signal, null);
    assert.equal(result.status, 0, result.stderr);
    const expectedRoot = setting === 'isolated' ? fixture.root : '/tmp';
    assert.equal(readFileSync(capture, 'utf8'), `${expectedRoot}/locus-ws.XXXXXX`);
    const workspace = readFileSync(observed, 'utf8');
    assert.equal(dirname(workspace), fixture.root);
    assert.throws(() => lstatSync(workspace), { code: 'ENOENT' });
  });
}

// A mktemp regression that relocates the workspace must stop the suite at the
// section 4 containment check; the exit trap still removes the misplaced
// directory. Runs the real workspace section, not a copy of its mktemp line.
test('a WS_DIR created outside TMPDIR fails the temporary-root check', (t) => {
  const fixture = setup(t);
  const scratch = join(fixture.root, 'custom tmp');
  const wrongRoot = join(fixture.root, 'wrong root');
  const created = join(fixture.root, 'ws-created');
  mkdirSync(scratch);
  mkdirSync(wrongRoot);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  const source = readFileSync(script, 'utf8');
  const cleanup = source.match(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m)?.[0];
  const createWorkspace = source.match(/^create_workspace\(\) \{[\s\S]*?^\}/m)?.[0];
  const start = source.indexOf('create_workspace\n# Same containment rule');
  const end = source.indexOf('\n# leave force-pin;', start);
  assert.ok(cleanup && createWorkspace && start >= 0 && end > start, 'missing workspace lifecycle');
  const probe = `set -euo pipefail
${cleanup}
${createWorkspace}
TMPDIR="$1"
LOCUS_HOME="$TMPDIR/main-home"
GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
mkdir "$LOCUS_HOME"
trap 'e2e_cleanup' EXIT
mktemp() { local d; d="$(command mktemp -d "$WRONG_ROOT/locus-ws.XXXXXX")"; printf '%s' "$d" > "$WS_CREATED"; printf '%s\\n' "$d"; }
locus() { return 74; }
log() { :; }; ok() { printf '%s\\n' "$*"; }; die() { printf '%s\\n' "$*" >&2; exit 76; }
${source.slice(start, end)}
`;
  const result = spawnSync('bash', ['-c', probe, 'workspace-misplaced', scratch], {
    env: { ...process.env, WRONG_ROOT: wrongRoot, WS_CREATED: created }, encoding: 'utf8',
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 76, `the check must fail the suite: ${result.stderr}`);
  assert.match(result.stderr, /WS_DIR was not created directly beneath TMPDIR/);
  assert.doesNotMatch(result.stdout, /workspace is created directly beneath TMPDIR/);
  const workspace = readFileSync(created, 'utf8');
  assert.equal(dirname(workspace), wrongRoot);
  assert.throws(() => lstatSync(workspace), { code: 'ENOENT' }, 'the misplaced workspace must be removed on exit');
  assert.deepEqual(readdirSync(scratch), ['keep']);
});

test('repeated workspace creation yields distinct directories beneath TMPDIR', (t) => {
  const fixture = setup(t);
  const scratch = join(fixture.root, 'custom tmp');
  const observed = join(fixture.root, 'observed-workspaces');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  const source = readFileSync(script, 'utf8');
  const cleanup = source.match(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m)?.[0];
  const createWorkspace = source.match(/^\s*WS_DIR="\$\(mktemp -d .*\)"$/m)?.[0];
  assert.ok(cleanup, 'missing e2e cleanup function');
  assert.ok(createWorkspace, 'missing workspace creation');

  const result = spawnSync('bash', ['-c', `set -euo pipefail
${cleanup}
TMPDIR="$1"
LOCUS_HOME="$TMPDIR/main-home"
mkdir "$LOCUS_HOME"
GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
trap 'e2e_cleanup' EXIT
${createWorkspace}
first="$WS_DIR"
${createWorkspace}
[[ -d "$first" && -d "$WS_DIR" && "$first" != "$WS_DIR" && ! "$first" -ef "$WS_DIR" ]] || exit 1
[[ "\${first%/*}" == "$TMPDIR" && "\${WS_DIR%/*}" == "$TMPDIR" && "\${first##*/}" == locus-ws.* && "\${WS_DIR##*/}" == locus-ws.* ]] || exit 1
touch "$first/sentinel" "$WS_DIR/sentinel"
printf '%s\\n%s\\n' "$first" "$WS_DIR" > "$2"
`, 'workspace-repeat', scratch, observed], { encoding: 'utf8' });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, 'two creations must produce distinct workspaces beneath TMPDIR');
  const [first, second] = readFileSync(observed, 'utf8').trim().split('\n');
  assert.notEqual(first, second, 'the second creation must not reuse the first directory');
  for (const workspace of [first, second]) {
    assert.equal(dirname(workspace), scratch, 'each workspace is created directly beneath the chosen TMPDIR');
    assert.match(workspace, /\/locus-ws\.[^/]+$/);
  }
  // The EXIT trap removes the live WS_DIR only; a superseded workspace is no
  // longer tracked, which is why the script creates one per run.
  assert.throws(() => lstatSync(second), { code: 'ENOENT' }, 'the live workspace must be removed on exit');
  assert.ok(existsSync(join(first, 'sentinel')), 'the superseded workspace is not tracked for removal');
  rmSync(first, { recursive: true, force: true });
  assert.deepEqual(readdirSync(scratch), ['keep'], 'cleanup must preserve unrelated files');
});

test('separate shells create distinct workspaces and each cleans its own', (t) => {
  const fixture = setup(t);
  const scratch = join(fixture.root, 'custom tmp');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  const source = readFileSync(script, 'utf8');
  const cleanup = source.match(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m)?.[0];
  const createWorkspace = source.match(/^\s*WS_DIR="\$\(mktemp -d .*\)"$/m)?.[0];
  assert.ok(cleanup, 'missing e2e cleanup function');
  assert.ok(createWorkspace, 'missing workspace creation');
  const probe = `set -euo pipefail
${cleanup}
TMPDIR="$1"
LOCUS_HOME="$TMPDIR/main-home"
mkdir "$LOCUS_HOME"
GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
trap 'e2e_cleanup' EXIT
${createWorkspace}
[[ -d "$WS_DIR" && "\${WS_DIR%/*}" == "$TMPDIR" && "\${WS_DIR##*/}" == locus-ws.* ]] || exit 1
touch "$WS_DIR/sentinel"
printf '%s' "$WS_DIR" > "$2"
`;

  const workspaces = [1, 2].map((n) => {
    const observed = join(fixture.root, `observed-workspace-${n}`);
    const result = spawnSync('bash', ['-c', probe, 'workspace-invocation', scratch, observed],
      { encoding: 'utf8' });
    assert.equal(result.signal, null);
    assert.equal(result.status, 0, `invocation ${n} must create its workspace beneath TMPDIR`);
    return readFileSync(observed, 'utf8');
  });

  const [first, second] = workspaces;
  assert.notEqual(first, second, 'each invocation must create a fresh directory');
  for (const workspace of workspaces) {
    assert.equal(dirname(workspace), scratch, 'each workspace is created directly beneath the chosen TMPDIR');
    assert.match(workspace, /\/locus-ws\.[^/]+$/);
    assert.throws(() => lstatSync(workspace), { code: 'ENOENT' }, `removed by its own exit trap: ${workspace}`);
  }
  assert.deepEqual(readdirSync(scratch), ['keep'], 'neither invocation may leave anything behind');
});

// Runs the script's real section 2d: a subshell rehearses create_workspace
// beneath a nested temporary root whose name contains spaces (including a
// doubled space), verifies the workspace sits directly beneath that root, and
// its exit trap removes it while sparing a decoy at the root's first
// word-split fragment. LOCUS_HOME is exported so the rehearsal's `init` stub
// populates the subshell home the way the real command would.
test('section 2d rehearses workspace creation beneath a TMPDIR containing spaces', (t) => {
  const fixture = setup(t);
  const home = join(fixture.root, 'main home');
  mkdirSync(home);
  const source = readFileSync(script, 'utf8');
  const cleanup = source.match(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m)?.[0];
  const createWorkspace = source.match(/^create_workspace\(\) \{[\s\S]*?^\}/m)?.[0];
  const start = source.indexOf('log "2d.');
  const endMarker = 'ok "workspace creation succeeds beneath a TMPDIR containing spaces"';
  const end = source.indexOf(endMarker, start);
  assert.ok(cleanup && createWorkspace && start >= 0 && end > start,
    'missing the section 2d workspace rehearsal');
  const result = spawnSync('bash', ['-c', `set -euo pipefail
${cleanup}
${createWorkspace}
log() { :; }
ok() { printf 'ok %s\\n' "$*"; }
die() { printf 'FAIL %s\\n' "$*" >&2; exit 1; }
export LOCUS_HOME="$1"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
${source.slice(start, end + endMarker.length)}
`, 'workspace-2d', home], {
    env: { ...process.env, LOCUS_BIN: fixture.locus },
    encoding: 'utf8',
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, `the 2d rehearsal must succeed: ${result.stderr}`);
  assert.match(result.stdout, /ok workspace creation succeeds beneath a TMPDIR containing spaces/);
  const root = join(home, 'workspace  ws temp root');
  const recorded = readFileSync(join(root, 'workspace-path'), 'utf8');
  assert.equal(dirname(recorded), root, 'rehearsed beneath the space-containing root');
  assert.match(recorded, /\/locus-ws\.[^/\s]+$/, 'workspace name is one unsplit component');
  assert.ok(recorded.includes(' '), 'the recorded workspace path keeps the spaced root');
  assert.throws(() => lstatSync(recorded), { code: 'ENOENT' },
    'the rehearsal workspace must be removed on subshell exit');
  assert.deepEqual(readdirSync(root), ['workspace-path'],
    'the subshell trap must also remove its own home');
  assert.ok(existsSync(join(home, 'workspace', 'decoy-sentinel')),
    'the decoy at the root\'s first word-split fragment must survive cleanup');
});

// A mktemp regression that relocates the rehearsal workspace must stop the
// suite at the 2d subshell; the subshell trap still removes the misplaced
// directory before the failure reaches the caller.
test('a rehearsal workspace created outside the spaced TMPDIR stops section 2d', (t) => {
  const fixture = setup(t);
  const home = join(fixture.root, 'main home');
  const wrongRoot = join(fixture.root, 'wrong root');
  const created = join(fixture.root, 'ws-created');
  mkdirSync(home);
  mkdirSync(wrongRoot);
  const source = readFileSync(script, 'utf8');
  const cleanup = source.match(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m)?.[0];
  const createWorkspace = source.match(/^create_workspace\(\) \{[\s\S]*?^\}/m)?.[0];
  const start = source.indexOf('log "2d.');
  const endMarker = 'ok "workspace creation succeeds beneath a TMPDIR containing spaces"';
  const end = source.indexOf(endMarker, start);
  assert.ok(cleanup && createWorkspace && start >= 0 && end > start,
    'missing the section 2d workspace rehearsal');
  const result = spawnSync('bash', ['-c', `set -euo pipefail
${cleanup}
mktemp() { local d; d="$(command mktemp -d "$WRONG_ROOT/locus-ws.XXXXXX")"; printf '%s' "$d" > "$WS_CREATED"; printf '%s\\n' "$d"; }
${createWorkspace}
log() { :; }
ok() { printf 'ok %s\\n' "$*"; }
die() { printf 'FAIL %s\\n' "$*" >&2; exit 1; }
LOCUS_HOME="$1"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
${source.slice(start, end + endMarker.length)}
`, 'workspace-2d-misplaced', home], {
    env: { ...process.env, LOCUS_BIN: fixture.locus, WRONG_ROOT: wrongRoot, WS_CREATED: created },
    encoding: 'utf8',
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 1, 'the 2d check must fail the suite');
  assert.match(result.stderr, /workspace was not created beneath the space-containing TMPDIR/);
  assert.doesNotMatch(result.stdout, /ok workspace creation succeeds/);
  assert.equal(readFileSync(created, 'utf8').startsWith(`${wrongRoot}/`), true);
  assert.deepEqual(readdirSync(wrongRoot), [], 'the misplaced rehearsal workspace must be removed');
});

for (const exitStatus of [0, 73]) {
  test(`security fixture directories are removed on exit ${exitStatus}`, (t) => {
    const fixture = setup(t);
    const scratch = join(fixture.root, 'custom tmp');
    mkdirSync(scratch);
    writeFileSync(join(scratch, 'keep'), 'unrelated');
    const source = readFileSync(script, 'utf8');
    const cleanup = source.match(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m)?.[0];
    const createHome = source.match(/^create_security_home\(\) \{[\s\S]*?^\}/m)?.[0];
    const createWorkspace = source.match(/^create_security_workspace\(\) \{[\s\S]*?^\}/m)?.[0];
    assert.ok(cleanup, 'missing e2e cleanup function');
    assert.ok(createHome, 'missing security home creation function');
    assert.ok(createWorkspace, 'missing security workspace creation');
    const probe = `${cleanup}
${createHome}
${createWorkspace}
TMPDIR="$1"
LOCUS_HOME="$TMPDIR/main-home"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
mkdir "$LOCUS_HOME"
trap 'e2e_cleanup' EXIT
create_security_home
create_security_workspace
[[ -d "$SECURITY_HOME" && -d "$SECURITY_WS" && ! "$SECURITY_HOME" -ef "$SECURITY_WS" && "\${SECURITY_HOME%/*}" == "$TMPDIR" && "\${SECURITY_WS%/*}" == "$TMPDIR" ]] || exit 1
touch "$SECURITY_HOME/sentinel" "$SECURITY_WS/sentinel"
exit "$2"
`;
    const result = spawnSync('bash', ['-c', probe, 'security-cleanup', scratch, String(exitStatus)], { encoding: 'utf8' });
    assert.equal(result.status, exitStatus, 'security fixture creation or cleanup probe failed');
    assert.equal(result.signal, null);
    assert.deepEqual(readdirSync(scratch), ['keep'], 'cleanup must remove fixtures and preserve unrelated files');
  });
}

test('an unhandled failure after security workspace creation triggers cleanup', (t) => {
  const fixture = setup(t);
  const scratch = join(fixture.root, 'custom tmp $literal; [test]');
  const record = join(fixture.root, 'security-workspace');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  const source = readFileSync(script, 'utf8');
  const cleanup = source.match(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m)?.[0];
  const createWorkspace = source.match(/^create_security_workspace\(\) \{[\s\S]*?^\}/m)?.[0];
  assert.ok(cleanup, 'missing e2e cleanup function');
  assert.ok(createWorkspace, 'missing security workspace creation function');
  const probe = `set -euo pipefail
${cleanup}
${createWorkspace}
TMPDIR="$1"
LOCUS_HOME="$TMPDIR/main-home"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
mkdir "$LOCUS_HOME"
trap 'e2e_cleanup' EXIT
create_security_workspace
[[ -d "$SECURITY_WS" && "\${SECURITY_WS%/*}" == "$TMPDIR" ]] || exit 91
touch "$SECURITY_WS/sentinel"
printf '%s' "$SECURITY_WS" > "$2"
(exit 73)
touch "$3"
`;
  const marker = join(fixture.root, 'continued');
  const result = spawnSync('bash', ['-c', probe, 'security-workspace-failure', scratch, record, marker],
    { encoding: 'utf8', timeout: 15_000 });
  assert.equal(result.signal, null);
  assert.equal(result.status, 73, 'the injected command failure must survive cleanup');
  const workspace = readFileSync(record, 'utf8');
  assert.equal(dirname(workspace), scratch);
  assert.match(workspace, /\/locus-security-ws\.[^/]+$/);
  assert.equal(existsSync(workspace), false, 'populated security workspace survived failure cleanup');
  assert.equal(existsSync(marker), false, 'execution continued after the injected failure');
  assert.deepEqual(readdirSync(scratch), ['keep'], 'cleanup removed or left unexpected files');
});

// Stubbed mktemp records the template the script's real WS_DIR line asks for,
// so nothing is created in the real /tmp for the fallback cases.
for (const [label, temp] of [['custom', 'custom tmp $literal; [test]'], ['unset', undefined], ['empty', '']]) {
  test(`workspace template uses ${label} TMPDIR root`, (t) => {
    const fixture = setup(t);
    const scratch = join(fixture.root, temp === undefined || temp === '' ? 'unused' : temp);
    const capture = join(fixture.root, 'template');
    mkdirSync(scratch);
    const createWorkspace = readFileSync(script, 'utf8').match(/^\s*WS_DIR="\$\(mktemp -d .*\)"$/m)?.[0];
    assert.ok(createWorkspace, 'missing workspace creation');
    writeFileSync(join(fixture.bin, 'mktemp'), `#!/bin/sh
printf '%s' "$2" > "$PROBE_CAPTURE"
printf '%s\\n' "\${2%XXXXXX}stub"
`, { mode: 0o755 });
    const env = { ...process.env, PATH: `${fixture.bin}:${process.env.PATH}`, PROBE_CAPTURE: capture };
    if (temp === undefined) delete env.TMPDIR;
    else env.TMPDIR = temp === '' ? '' : scratch;
    const result = spawnSync('bash', ['-c', `set -euo pipefail\n${createWorkspace}\nprintf '%s' "$WS_DIR"`],
      { env, encoding: 'utf8' });

    assert.equal(result.status, 0, result.stderr);
    const root = temp ? scratch : '/tmp';
    assert.equal(readFileSync(capture, 'utf8'), `${root}/locus-ws.XXXXXX`);
    assert.equal(result.stdout, `${root}/locus-ws.stub`);
  });
}

// A stubbed mktemp counts its arguments: a space-containing TMPDIR must reach
// it inside one quoted template, never split into several words.
for (const [label, temp] of [['spaced', 'spaced tmp dir'], ['unset', undefined], ['empty', '']]) {
  test(`security workspace template is one mktemp argument with ${label} TMPDIR`, (t) => {
    const fixture = setup(t);
    const scratch = join(fixture.root, temp || 'unused');
    const capture = join(fixture.root, 'args');
    mkdirSync(scratch);
    const fn = readFileSync(script, 'utf8')
      .match(/^create_security_workspace\(\) \{\n[^\n]*\n\}$/m)?.[0];
    assert.ok(fn, 'missing create_security_workspace');
    writeFileSync(join(fixture.bin, 'mktemp'), `#!/bin/sh
printf '%s' "$#" > "$PROBE_CAPTURE.argc"
printf '%s' "$1" > "$PROBE_CAPTURE.flag"
printf '%s' "$2" > "$PROBE_CAPTURE.template"
printf '%s\\n' "\${2%XXXXXX}stub"
`, { mode: 0o755 });
    const env = { ...process.env, PATH: `${fixture.bin}:${process.env.PATH}`, PROBE_CAPTURE: capture };
    if (temp === undefined) delete env.TMPDIR;
    else env.TMPDIR = temp === '' ? '' : scratch;
    const result = spawnSync('bash', ['-c', `set -euo pipefail\n${fn}\ncreate_security_workspace\nprintf '%s' "$SECURITY_WS"`],
      { env, encoding: 'utf8' });

    assert.equal(result.status, 0, result.stderr);
    const base = temp ? scratch : '/tmp';
    assert.equal(readFileSync(`${capture}.argc`, 'utf8'), '2', 'mktemp must receive exactly -d and one template');
    assert.equal(readFileSync(`${capture}.flag`, 'utf8'), '-d');
    assert.equal(readFileSync(`${capture}.template`, 'utf8'), `${base}/locus-security-ws.XXXXXX`);
    assert.equal(result.stdout, `${base}/locus-security-ws.stub`);
  });
}

test('security home is created beneath a TMPDIR containing spaces and removed on exit', (t) => {
  const fixture = setup(t);
  const scratch = join(fixture.root, 'scratch dir with  spaces');
  const observed = join(fixture.root, 'observed-security-home');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  const source = readFileSync(script, 'utf8');
  const cleanup = source.match(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m)?.[0];
  const createHome = source.match(/^create_security_home\(\) \{[\s\S]*?^\}/m)?.[0];
  assert.ok(cleanup && createHome, 'missing cleanup or security home creation');

  const result = spawnSync('bash', ['-c', `set -euo pipefail
${cleanup}
${createHome}
LOCUS_HOME="$1/main-home"
mkdir "$LOCUS_HOME"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
trap 'e2e_cleanup' EXIT
create_security_home
[[ -d "$SECURITY_HOME" && "\${SECURITY_HOME%/*}" == "$TMPDIR" && "\${SECURITY_HOME##*/}" == locus-security-e2e.* ]]
touch "$SECURITY_HOME/sentinel"
printf '%s' "$SECURITY_HOME" > "$2"
`, 'security-home-spaces', scratch, observed], {
    env: { ...process.env, TMPDIR: scratch },
    encoding: 'utf8',
  });

  assert.equal(result.status, 0, result.stderr);
  const home = readFileSync(observed, 'utf8');
  assert.equal(dirname(home), scratch, 'created at the intended spaced path');
  assert.match(home, /\/locus-security-e2e\.[^/]+$/);
  assert.throws(() => lstatSync(home), { code: 'ENOENT' }, 'security home must be removed on exit');
  assert.deepEqual(readdirSync(scratch), ['keep'], 'cleanup must preserve unrelated files');
});

test('security workspace is created beneath a TMPDIR containing spaces as a single path and removed on exit', (t) => {
  const fixture = setup(t);
  const scratch = join(fixture.root, 'scratch dir with  spaces');
  const observed = join(fixture.root, 'observed-security-ws');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  const source = readFileSync(script, 'utf8');
  const cleanup = source.match(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m)?.[0];
  const createWs = source.match(/^create_security_workspace\(\) \{[\s\S]*?^\}/m)?.[0];
  assert.ok(cleanup && createWs, 'missing cleanup or security workspace creation');

  const result = spawnSync('bash', ['-c', `set -euo pipefail
${cleanup}
${createWs}
LOCUS_HOME="$1/main-home"
mkdir "$LOCUS_HOME"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
trap 'e2e_cleanup' EXIT
create_security_workspace
[[ -d "$SECURITY_WS" && "\${SECURITY_WS%/*}" == "$TMPDIR" && "\${SECURITY_WS##*/}" == locus-security-ws.* ]]
touch "$SECURITY_WS/sentinel"
printf '%s' "$SECURITY_WS" > "$2"
`, 'security-ws-spaces', scratch, observed], {
    env: { ...process.env, TMPDIR: scratch },
    encoding: 'utf8',
  });

  assert.equal(result.status, 0, result.stderr);
  const ws = readFileSync(observed, 'utf8');
  assert.equal(dirname(ws), scratch, 'created directly beneath the spaced TMPDIR');
  assert.match(ws, /\/locus-security-ws\.[^/\s]+$/, 'final component is one unsplit name');
  assert.throws(() => lstatSync(ws), { code: 'ENOENT' }, 'security workspace must be removed on exit');
  assert.deepEqual(readdirSync(scratch), ['keep'], 'cleanup must preserve unrelated files and create no split paths');
});

test('a security workspace created outside TMPDIR fails the temporary-root check', (t) => {
  const fixture = setup(t);
  const scratch = join(fixture.root, 'custom tmp');
  const wrongRoot = join(fixture.root, 'wrong root');
  const created = join(fixture.root, 'security-ws-created');
  mkdirSync(scratch);
  mkdirSync(wrongRoot);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  const realMktemp = execFileSync('which', ['mktemp'], { encoding: 'utf8' }).trim();
  // Answer with the expected name shape but relocate the workspace outside
  // TMPDIR; the section 2c containment check must stop the suite before the
  // security probes run. The subshell exit trap still removes the misplaced
  // workspace and the parent trap removes the suite home.
  writeFileSync(join(fixture.bin, 'mktemp'), `#!/bin/sh
case "$2" in
  */locus-security-ws.XXXXXX)
    ws="$("$REAL_MKTEMP" -d "$WRONG_ROOT/locus-security-ws.XXXXXX")" || exit $?
    touch "$ws/sentinel" || exit 1
    printf '%s' "$ws" > "$WS_CREATED" || exit 1
    printf '%s\\n' "$ws"
    ;;
  *) exec "$REAL_MKTEMP" "$@" ;;
esac
`, { mode: 0o755 });

  const result = run(fixture, scratch, {
    REAL_MKTEMP: realMktemp, WRONG_ROOT: wrongRoot, WS_CREATED: created,
  });

  assert.equal(result.status, 1);
  assert.equal(result.signal, null);
  assert.match(result.stdout, /==> 2c\. security directories honor a nondefault TMPDIR/);
  assert.doesNotMatch(result.stdout, /ok  distinct security directories honor nondefault TMPDIR/);
  assert.match(result.stderr, /FAIL security directories were not created distinctly beneath the supplied TMPDIR/);
  assert.ok(existsSync(created), 'the relocated workspace creation was not reached');
  const workspace = readFileSync(created, 'utf8');
  assert.equal(dirname(workspace), wrongRoot);
  assert.match(workspace, /\/locus-security-ws\.[^/]+$/);
  assert.throws(() => lstatSync(workspace), { code: 'ENOENT' },
    'the misplaced workspace must be removed on exit');
  assert.deepEqual(readdirSync(wrongRoot), []);
  assert.deepEqual(readdirSync(scratch), ['keep']);
});

test('a security home created outside a spaced TMPDIR fails the temporary-root check', (t) => {
  const fixture = setup(t);
  const scratch = join(fixture.root, 'custom tmp with  spaces');
  const wrongRoot = join(fixture.root, 'wrong root');
  const created = join(fixture.root, 'security-home-created');
  mkdirSync(scratch);
  mkdirSync(wrongRoot);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  const realMktemp = execFileSync('which', ['mktemp'], { encoding: 'utf8' }).trim();
  // Same relocation as the workspace case: right name shape, wrong parent. The
  // section 2c check must stop the suite and the exit trap must remove the home.
  writeFileSync(join(fixture.bin, 'mktemp'), `#!/bin/sh
case "$2" in
  */locus-security-e2e.XXXXXX)
    home="$("$REAL_MKTEMP" -d "$WRONG_ROOT/locus-security-e2e.XXXXXX")" || exit $?
    touch "$home/sentinel" || exit 1
    printf '%s' "$home" > "$HOME_CREATED" || exit 1
    printf '%s\\n' "$home"
    ;;
  *) exec "$REAL_MKTEMP" "$@" ;;
esac
`, { mode: 0o755 });

  const result = run(fixture, scratch, {
    REAL_MKTEMP: realMktemp, WRONG_ROOT: wrongRoot, HOME_CREATED: created,
  });

  assert.equal(result.status, 1);
  assert.equal(result.signal, null);
  assert.match(result.stdout, /==> 2c\. security directories honor a nondefault TMPDIR/);
  assert.doesNotMatch(result.stdout, /ok  distinct security directories honor nondefault TMPDIR/);
  assert.match(result.stderr, /FAIL security directories were not created distinctly beneath the supplied TMPDIR/);
  assert.ok(existsSync(created), 'the relocated home creation was not reached');
  const home = readFileSync(created, 'utf8');
  assert.equal(dirname(home), wrongRoot);
  assert.match(home, /\/locus-security-e2e\.[^/]+$/);
  assert.throws(() => lstatSync(home), { code: 'ENOENT' },
    'the misplaced security home must be removed on exit');
  assert.deepEqual(readdirSync(wrongRoot), []);
  assert.deepEqual(readdirSync(scratch), ['keep']);
});

test('repeated security home creation yields distinct directories beneath TMPDIR', (t) => {
  const fixture = setup(t);
  const scratch = join(fixture.root, 'custom tmp');
  const observed = join(fixture.root, 'observed-security-homes');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  const source = readFileSync(script, 'utf8');
  const cleanup = source.match(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m)?.[0];
  const createHome = source.match(/^create_security_home\(\) \{[\s\S]*?^\}/m)?.[0];
  assert.ok(cleanup, 'missing e2e cleanup function');
  assert.ok(createHome, 'missing security home creation function');

  const result = spawnSync('bash', ['-c', `set -euo pipefail
${cleanup}
${createHome}
TMPDIR="$1"
LOCUS_HOME="$TMPDIR/main-home"
mkdir "$LOCUS_HOME"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
trap 'e2e_cleanup' EXIT
create_security_home
first="$SECURITY_HOME"
create_security_home
[[ -d "$first" && -d "$SECURITY_HOME" && "$first" != "$SECURITY_HOME" && ! "$first" -ef "$SECURITY_HOME" ]] || exit 1
[[ "\${first%/*}" == "$TMPDIR" && "\${SECURITY_HOME%/*}" == "$TMPDIR" && "\${first##*/}" == locus-security-e2e.* && "\${SECURITY_HOME##*/}" == locus-security-e2e.* ]] || exit 1
touch "$first/sentinel" "$SECURITY_HOME/sentinel"
printf '%s\\n%s\\n' "$first" "$SECURITY_HOME" > "$2"
`, 'security-home-repeat', scratch, observed], { encoding: 'utf8' });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, 'two invocations must create distinct security homes beneath TMPDIR');
  const [first, second] = readFileSync(observed, 'utf8').trim().split('\n');
  assert.notEqual(first, second, 'the second invocation must not reuse the first directory');
  for (const home of [first, second]) {
    assert.equal(dirname(home), scratch, 'each home is created directly beneath the chosen TMPDIR');
    assert.match(home, /\/locus-security-e2e\.[^/]+$/);
  }
  // The EXIT trap removes the live SECURITY_HOME only; a superseded home is no
  // longer tracked, which is why the script creates one per shell.
  assert.throws(() => lstatSync(second), { code: 'ENOENT' }, 'the live security home must be removed on exit');
  assert.ok(existsSync(join(first, 'sentinel')), 'the superseded home is not tracked for removal');
  rmSync(first, { recursive: true, force: true });
  assert.deepEqual(readdirSync(scratch), ['keep'], 'cleanup must preserve unrelated files');
});

test('separate shells create distinct security homes and each cleans its own', (t) => {
  const fixture = setup(t);
  const scratch = join(fixture.root, 'custom tmp');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  const source = readFileSync(script, 'utf8');
  const cleanup = source.match(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m)?.[0];
  const createHome = source.match(/^create_security_home\(\) \{[\s\S]*?^\}/m)?.[0];
  assert.ok(cleanup, 'missing e2e cleanup function');
  assert.ok(createHome, 'missing security home creation function');
  const probe = `set -euo pipefail
${cleanup}
${createHome}
TMPDIR="$1"
LOCUS_HOME="$TMPDIR/main-home"
mkdir "$LOCUS_HOME"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
trap 'e2e_cleanup' EXIT
create_security_home
[[ -d "$SECURITY_HOME" && "\${SECURITY_HOME%/*}" == "$TMPDIR" && "\${SECURITY_HOME##*/}" == locus-security-e2e.* ]] || exit 1
touch "$SECURITY_HOME/sentinel"
printf '%s' "$SECURITY_HOME" > "$2"
`;

  const homes = [1, 2].map((n) => {
    const observed = join(fixture.root, `observed-home-${n}`);
    const result = spawnSync('bash', ['-c', probe, 'security-home-invocation', scratch, observed],
      { encoding: 'utf8' });
    assert.equal(result.signal, null);
    assert.equal(result.status, 0, `invocation ${n} must create its home beneath TMPDIR`);
    return readFileSync(observed, 'utf8');
  });

  const [first, second] = homes;
  assert.notEqual(first, second, 'each invocation must create a fresh directory');
  for (const home of homes) {
    assert.equal(dirname(home), scratch, 'each home is created directly beneath the chosen TMPDIR');
    assert.match(home, /\/locus-security-e2e\.[^/]+$/);
    assert.throws(() => lstatSync(home), { code: 'ENOENT' }, `removed by its own exit trap: ${home}`);
  }
  assert.deepEqual(readdirSync(scratch), ['keep'], 'neither invocation may leave anything behind');
});

test('repeated security workspace creation yields distinct directories beneath a spaced TMPDIR', (t) => {
  const fixture = setup(t);
  const scratch = join(fixture.root, 'security ws tmp $literal; [test]');
  const observed = join(fixture.root, 'observed-security-workspaces');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  const source = readFileSync(script, 'utf8');
  const cleanup = source.match(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m)?.[0];
  const createWs = source.match(/^create_security_workspace\(\) \{[\s\S]*?^\}/m)?.[0];
  assert.ok(cleanup, 'missing e2e cleanup function');
  assert.ok(createWs, 'missing security workspace creation function');

  // The script invokes create_security_workspace once per shell; a second call
  // must still land beneath the spaced TMPDIR as a fresh directory.
  const result = spawnSync('bash', ['-c', `set -euo pipefail
${cleanup}
${createWs}
TMPDIR="$1"
LOCUS_HOME="$TMPDIR/main-home"
mkdir "$LOCUS_HOME"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
trap 'e2e_cleanup' EXIT
create_security_workspace
first="$SECURITY_WS"
create_security_workspace
[[ -d "$first" && -d "$SECURITY_WS" && "$first" != "$SECURITY_WS" && ! "$first" -ef "$SECURITY_WS" ]] || exit 1
[[ "\${first%/*}" == "$TMPDIR" && "\${SECURITY_WS%/*}" == "$TMPDIR" && "\${first##*/}" == locus-security-ws.* && "\${SECURITY_WS##*/}" == locus-security-ws.* ]] || exit 1
touch "$first/sentinel" "$SECURITY_WS/sentinel"
printf '%s\\n%s\\n' "$first" "$SECURITY_WS" > "$2"
`, 'security-ws-repeat', scratch, observed], { encoding: 'utf8' });

  assert.equal(result.signal, null);
  assert.equal(result.status, 0, 'two invocations must create distinct workspaces beneath the spaced TMPDIR');
  const [first, second] = readFileSync(observed, 'utf8').trim().split('\n');
  assert.notEqual(first, second, 'the second invocation must not reuse the first directory');
  for (const workspace of [first, second]) {
    assert.equal(dirname(workspace), scratch, 'each workspace is created directly beneath the spaced TMPDIR');
    assert.match(workspace, /\/locus-security-ws\.[^/]+$/);
  }
  // The EXIT trap removes the live SECURITY_WS only; a superseded workspace is
  // no longer tracked, which is why the script creates one per shell.
  assert.throws(() => lstatSync(second), { code: 'ENOENT' }, 'the live security workspace must be removed on exit');
  assert.ok(existsSync(join(first, 'sentinel')), 'the superseded workspace is not tracked for removal');
  rmSync(first, { recursive: true, force: true });
  assert.deepEqual(readdirSync(scratch), ['keep'], 'cleanup must preserve unrelated files');
});

test('separate shells create distinct security workspaces beneath a spaced TMPDIR and each cleans its own', (t) => {
  const fixture = setup(t);
  const scratch = join(fixture.root, 'security ws tmp dir');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  const source = readFileSync(script, 'utf8');
  const cleanup = source.match(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m)?.[0];
  const createWs = source.match(/^create_security_workspace\(\) \{[\s\S]*?^\}/m)?.[0];
  assert.ok(cleanup, 'missing e2e cleanup function');
  assert.ok(createWs, 'missing security workspace creation function');
  const probe = `set -euo pipefail
${cleanup}
${createWs}
TMPDIR="$1"
LOCUS_HOME="$TMPDIR/main-home"
mkdir "$LOCUS_HOME"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
trap 'e2e_cleanup' EXIT
create_security_workspace
[[ -d "$SECURITY_WS" && "\${SECURITY_WS%/*}" == "$TMPDIR" && "\${SECURITY_WS##*/}" == locus-security-ws.* ]] || exit 1
touch "$SECURITY_WS/sentinel"
printf '%s' "$SECURITY_WS" > "$2"
`;

  const workspaces = [1, 2].map((n) => {
    const observed = join(fixture.root, `observed-workspace-${n}`);
    const result = spawnSync('bash', ['-c', probe, 'security-ws-invocation', scratch, observed],
      { encoding: 'utf8' });
    assert.equal(result.signal, null);
    assert.equal(result.status, 0, `invocation ${n} must create its workspace beneath the spaced TMPDIR`);
    return readFileSync(observed, 'utf8');
  });

  const [first, second] = workspaces;
  assert.notEqual(first, second, 'each invocation must create a fresh directory');
  for (const workspace of workspaces) {
    assert.equal(dirname(workspace), scratch, 'each workspace is created directly beneath the spaced TMPDIR');
    assert.match(workspace, /\/locus-security-ws\.[^/]+$/);
    assert.throws(() => lstatSync(workspace), { code: 'ENOENT' }, `removed by its own exit trap: ${workspace}`);
  }
  assert.deepEqual(readdirSync(scratch), ['keep'], 'neither invocation may leave anything behind');
});

test('repeated security workspace block yields distinct directories beneath TMPDIR', (t) => {
  const fixture = setup(t);
  const scratch = join(fixture.root, 'custom tmp $literal; [test]');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  const source = readFileSync(script, 'utf8');
  const create = source.match(/^create_security_workspace\(\) \{[\s\S]*?^\}/m)?.[0];
  const block = source.match(/^security_ws_live="\$SECURITY_WS"[\s\S]*?^ok "two security workspaces are distinct[^\n]*$/m)?.[0];
  assert.ok(create && block, 'missing repeated security workspace block');

  for (const tmpdir of [scratch, `${scratch}/`]) {
    const result = spawnSync('bash', ['-c', `set -euo pipefail
die() { printf 'FAIL %s\\n' "$*" >&2; exit 1; }
ok() { printf 'ok %s\\n' "$*"; }
${create}
SECURITY_WS=""
${block}
`], { env: { ...process.env, TMPDIR: tmpdir }, encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /two security workspaces are distinct/);
    assert.deepEqual(readdirSync(scratch), ['keep'], 'block must clean up both workspaces');
  }
});

test('graph import stops clearly when its temporary home cannot be created', (t) => {
  const fixture = setup(t);
  const home = join(fixture.root, 'main-home');
  const calls = join(fixture.root, 'calls');
  mkdirSync(home);
  const source = readFileSync(script, 'utf8');
  const create = source.match(/^create_graph_home\(\) \{[\s\S]*?^\}/m)?.[0];
  const graph = source.slice(source.indexOf('# ── 16. graph'), source.indexOf('# ── 17. ci'));
  assert.ok(create && graph.includes('create_graph_home'), 'missing graph import code');

  for (const failure of ['error', 'empty']) {
    rmSync(calls, { force: true });
    const result = spawnSync('bash', ['-c', `set -euo pipefail
GRAPH_HOME=""
log() { :; }
ok() { :; }
skip() { exit 90; }
die() { printf 'FAIL %s\\n' "$*" >&2; exit 1; }
has_cmd() { return 0; }
mktemp() { [[ "$TEST_FAILURE" == empty ]]; }
locus() {
  printf '%s:%s\\n' "$1" "\${2:-}" >> "$TEST_CALLS"
  case "$1:\${2:-}" in
    graph:export) printf 'LOCUSGRAPH00' > "$4" ;;
    *) return 1 ;;
  esac
}
${create}
${graph}
`], { env: { ...process.env, LOCUS_HOME: home, TEST_CALLS: calls, TEST_FAILURE: failure }, encoding: 'utf8' });
    assert.equal(result.status, 1, failure);
    assert.match(result.stderr, /could not create GRAPH_HOME for graph import/);
    assert.deepEqual(readFileSync(calls, 'utf8').trim().split('\n'),
      ['graph:export', 'graph:import', 'graph:import'], 'no init or fresh-home import may run');
  }
});

test('graph home allocation failure removes earlier temporary directories', (t) => {
  const fixture = setup(t);
  const scratch = join(fixture.root, 'custom tmp $literal; [test]');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  const source = readFileSync(script, 'utf8');
  const cleanup = source.match(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m)?.[0];
  const create = source.match(/^create_graph_home\(\) \{[\s\S]*?^\}/m)?.[0];
  const graph = source.slice(source.indexOf('# ── 16. graph'), source.indexOf('# ── 17. ci'));
  assert.ok(cleanup && create && graph.includes('create_graph_home'), 'missing graph import code');

  // Earlier suite directories exist and are populated when the graph import
  // home allocation fails; the EXIT trap must remove every one of them.
  const earlier = {
    LOCUS_HOME: 'locus-e2e.earlier',
    WS_DIR: 'locus-ws.earlier',
    SECURITY_HOME: 'locus-security-e2e.earlier',
    SECURITY_WS: 'locus-security-ws.earlier',
  };
  for (const name of Object.values(earlier)) {
    mkdirSync(join(scratch, name));
    writeFileSync(join(scratch, name, 'sentinel'), name);
  }

  const result = spawnSync('bash', ['-c', `set -euo pipefail
${cleanup}
GRAPH_HOME=""
HTTP_MCP_PID="" MT_MCP_PID=""
LOCUS_HOME="$TMPDIR/${earlier.LOCUS_HOME}"
WS_DIR="$TMPDIR/${earlier.WS_DIR}"
SECURITY_HOME="$TMPDIR/${earlier.SECURITY_HOME}"
SECURITY_WS="$TMPDIR/${earlier.SECURITY_WS}"
log() { :; }
ok() { :; }
skip() { exit 90; }
die() { printf 'FAIL %s\\n' "$*" >&2; exit 1; }
has_cmd() { return 0; }
mktemp() {
  case "$2" in
    */locus-graph-imp.XXXXXX) printf 'injected graph home failure\\n' >&2; return 73 ;;
  esac
  command mktemp "$@"
}
locus() {
  case "$1:\${2:-}" in
    graph:export) printf 'LOCUSGRAPH00' > "$4" ;;
    *) return 1 ;;
  esac
}
trap 'e2e_cleanup' EXIT
${create}
${graph}
`], { env: { ...process.env, TMPDIR: scratch }, encoding: 'utf8' });

  assert.equal(result.signal, null);
  assert.equal(result.status, 1, 'the allocation failure must stop the suite');
  assert.match(result.stderr, /injected graph home failure/);
  assert.match(result.stderr, /could not create GRAPH_HOME for graph import/);
  assert.deepEqual(readdirSync(scratch), ['keep'],
    'every earlier temporary directory must be removed on failure');
});

test('graph home creation selects the custom temporary root or /tmp fallback', (t) => {
  const fixture = setup(t);
  const scratch = join(fixture.root, 'custom tmp $literal; [test]');
  mkdirSync(scratch);
  const source = readFileSync(script, 'utf8');
  const create = source.match(/^create_graph_home\(\) \{[\s\S]*?^\}/m)?.[0];
  assert.ok(create, 'missing graph home creation function');

  for (const [label, value, expectedRoot] of [
    ['custom', scratch, scratch],
    ['unset', undefined, '/tmp'],
    ['empty', '', '/tmp'],
  ]) {
    const env = { ...process.env };
    if (value === undefined) delete env.TMPDIR;
    else env.TMPDIR = value;
    const result = spawnSync('bash', ['-c', `set -euo pipefail
${create}
create_graph_home
trap 'rm -rf "$GRAPH_HOME"' EXIT
[[ -d "$GRAPH_HOME" && ! -L "$GRAPH_HOME" ]]
printf '%s' "$GRAPH_HOME"
`], { env, encoding: 'utf8' });
    assert.equal(result.status, 0, `${label}: creation must succeed`);
    assert.equal(dirname(result.stdout), expectedRoot, `${label}: selected root`);
    assert.match(result.stdout, /\/locus-graph-imp\.[^/]+$/);
    assert.throws(() => lstatSync(result.stdout), { code: 'ENOENT' }, `${label}: cleanup`);
  }
});

test('repeated graph home creation yields distinct directories beneath TMPDIR', (t) => {
  const fixture = setup(t);
  const scratch = join(fixture.root, 'custom tmp $literal; [test]');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  const source = readFileSync(script, 'utf8');
  const create = source.match(/^create_graph_home\(\) \{[\s\S]*?^\}/m)?.[0];
  const block = source.match(/^create_graph_home; graph_home_a=[\s\S]*?^ok "two graph homes are distinct[^\n]*$/m)?.[0];
  assert.ok(create && block, 'missing repeated graph home block');

  for (const tmpdir of [scratch, `${scratch}/`]) {
    const result = spawnSync('bash', ['-c', `set -euo pipefail
die() { printf 'FAIL %s\\n' "$*" >&2; exit 1; }
ok() { printf 'ok %s\\n' "$*"; }
${create}
GRAPH_HOME=""
${block}
`], { env: { ...process.env, TMPDIR: tmpdir }, encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /two graph homes are distinct/);
    assert.deepEqual(readdirSync(scratch), ['keep'], 'block must clean up both homes');
  }
});

// Real mktemp, no stub: with TMPDIR unset the security workspace must actually
// be created as a directory directly under /tmp (the documented fallback).
test('security workspace is created under /tmp when TMPDIR is unset', (t) => {
  const fn = readFileSync(script, 'utf8')
    .match(/^create_security_workspace\(\) \{\n[^\n]*\n\}$/m)?.[0];
  assert.ok(fn, 'missing create_security_workspace');
  const env = { ...process.env };
  delete env.TMPDIR;
  const result = spawnSync('bash', ['-c', `set -euo pipefail\n${fn}\ncreate_security_workspace\nprintf '%s' "$SECURITY_WS"`],
    { env, encoding: 'utf8' });
  const created = result.stdout;
  if (created.startsWith('/tmp/')) t.after(() => rmSync(created, { recursive: true, force: true }));

  assert.equal(result.status, 0, result.stderr);
  assert.match(created, /^\/tmp\/locus-security-ws\.[A-Za-z0-9]{6}$/);
  assert.ok(lstatSync(created).isDirectory(), 'workspace must exist as a real directory');
});

// WS_DIR must reach every command as exactly one argument, even when TMPDIR has
// a doubled space and glob characters that word splitting or globbing would
// mangle. Static: every expansion in the script is double-quoted. Dynamic: the
// real workspace section runs with `cd` and `rm` wrapped to record their argv.
test('WS_DIR under a space-containing TMPDIR is passed as one argument', (t) => {
  const source = readFileSync(script, 'utf8');
  const unquoted = source.split('\n').flatMap((line, i) => {
    if (!/\$\{?WS_DIR\b/.test(line) || line.trimStart().startsWith('#')) return [];
    const quoted = line.replace(/"[^"]*"/g, '');
    return /\$\{?WS_DIR\b/.test(quoted) ? [`${i + 1}: ${line.trim()}`] : [];
  });
  assert.deepEqual(unquoted, [], 'every WS_DIR expansion must be double-quoted');

  const fixture = setup(t);
  const scratch = join(fixture.root, 'temp  with *spaces? [x]');
  const argvLog = join(fixture.root, 'argv-log');
  mkdirSync(scratch);
  const cleanup = source.match(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m)?.[0];
  const createWorkspace = source.match(/^create_workspace\(\) \{[\s\S]*?^\}/m)?.[0];
  const start = source.indexOf('create_workspace\n# Same containment rule');
  const end = source.indexOf('\n# leave force-pin;', start);
  assert.ok(cleanup && createWorkspace && start >= 0 && end > start, 'missing workspace lifecycle');
  const probe = `set -euo pipefail
${cleanup}
${createWorkspace}
TMPDIR="$1"
LOCUS_HOME="$TMPDIR/main-home"
GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
mkdir "$LOCUS_HOME"
trap 'e2e_cleanup' EXIT
record() { local c="$1"; shift; printf '%s\\t%s' "$c" "$#" >> "$ARGV_LOG"; printf '\\t%s' "$@" >> "$ARGV_LOG"; printf '\\n' >> "$ARGV_LOG"; }
cd() { record cd "$@"; builtin cd "$@"; }
rm() { record rm "$@"; command rm "$@"; }
locus() {
  case "$*" in
    workspace*) touch .locus.toml ;;
    'pin personal') printf 'not allowed\\n' >&2; return 1 ;;
    'pin personal --force') return 0 ;;
    *) return 74 ;;
  esac
}
log() { :; }; ok() { :; }; die() { printf '%s\\n' "$*" >&2; exit 74; }
${source.slice(start, end)}
`;
  const result = spawnSync('bash', ['-c', probe, 'workspace-argv', scratch], {
    env: { ...process.env, ARGV_LOG: argvLog }, encoding: 'utf8',
  });
  assert.equal(result.signal, null);
  assert.equal(result.status, 0, `the workspace section must succeed: ${result.stderr}`);

  const rows = readFileSync(argvLog, 'utf8').trim().split('\n').map((l) => l.split('\t'));
  const cd = rows.find(([cmd, , dir]) => cmd === 'cd' && dir?.startsWith(`${scratch}/locus-ws.`));
  assert.ok(cd, 'the section must cd into the workspace');
  assert.equal(cd[1], '1', 'cd must receive WS_DIR as exactly one argument');
  const rm = rows.find(([cmd, , , ...rest]) => cmd === 'rm' && rest.some((a) => a.startsWith(`${scratch}/locus-ws.`)));
  assert.ok(rm, 'exit cleanup must remove the workspace');
  assert.ok(rm.slice(2).includes(cd[2]), 'cleanup must pass the same WS_DIR as one argument');
  assert.throws(() => lstatSync(cd[2]), { code: 'ENOENT' }, 'workspace must be removed');
  assert.deepEqual(readdirSync(scratch), [], 'successful cleanup must leave no temporary artifacts');
});

// Whitespace at the edges of a path component is the easiest thing for an
// unquoted expansion or a trimming command substitution to lose.
test('WS_DIR preserves the complete spaced TMPDIR path, including edge spaces', (t) => {
  const fixture = setup(t);
  const outer = join(fixture.root, ' lead  dir ');
  const scratch = join(outer, 'trail ');
  mkdirSync(scratch, { recursive: true });
  const record = join(fixture.root, 'ws-path');
  const source = readFileSync(script, 'utf8');
  const createWorkspace = source.match(/^create_workspace\(\) \{[\s\S]*?^\}/m)?.[0];
  assert.ok(createWorkspace, 'missing create_workspace');
  const result = spawnSync('bash', ['-c', `set -euo pipefail
${createWorkspace}
TMPDIR="$1"
create_workspace
[[ -d "$WS_DIR" && "\${WS_DIR%/*}" == "$TMPDIR" ]] || exit 74
printf '%s' "$WS_DIR" > "$RECORD"
`, 'ws-edge', scratch], { env: { ...process.env, RECORD: record }, encoding: 'utf8' });
  assert.equal(result.status, 0, result.stderr);
  const ws = readFileSync(record, 'utf8');
  t.after(() => rmSync(ws, { recursive: true, force: true }));
  assert.equal(dirname(ws), scratch, 'every character of the TMPDIR prefix survives');
  assert.match(ws.slice(scratch.length + 1), /^locus-ws\.[A-Za-z0-9]{6}$/);
  assert.ok(lstatSync(ws).isDirectory());
});

// Exercise all five allocation helpers together with a real mktemp. The shim
// rejects split templates before allocation and records each resulting path.
test('all temporary directory helpers preserve a spaced parent as one argument', (t) => {
  const fixture = setup(t);
  const scratch = join(fixture.root, 'parent with  spaces');
  const record = join(fixture.root, 'allocations');
  mkdirSync(scratch);
  const source = readFileSync(script, 'utf8');
  const names = ['create_locus_home', 'create_workspace', 'create_security_home',
    'create_security_workspace', 'create_graph_home'];
  const functions = names.map((name) => {
    const definition = source.match(new RegExp(`^${name}\\(\\) \\{[\\s\\S]*?^\\}`, 'm'))?.[0];
    assert.ok(definition, `missing ${name}`);
    return definition;
  }).join('\n');
  const realMktemp = execFileSync('which', ['mktemp'], { encoding: 'utf8' }).trim();
  writeFileSync(join(fixture.bin, 'mktemp'), `#!/bin/bash
set -eu
[[ "$#" == 2 && "$1" == -d && "\${2%/*}" == "$TMPDIR" ]] || exit 71
created="$("$REAL_MKTEMP" "$@")"
[[ -d "$created" && "\${created%/*}" == "$TMPDIR" ]] || exit 72
printf '%s\\n' "$created" >> "$RECORD"
printf '%s\\n' "$created"
`, { mode: 0o755 });
  const result = spawnSync('bash', ['-c', `set -euo pipefail
${functions}
${names.join('\n')}
`], { env: { ...process.env, PATH: `${fixture.bin}:${process.env.PATH}`,
    TMPDIR: scratch, REAL_MKTEMP: realMktemp, RECORD: record }, encoding: 'utf8' });
  assert.equal(result.status, 0, result.stderr);
  const paths = readFileSync(record, 'utf8').trim().split('\n');
  assert.equal(paths.length, names.length);
  assert.equal(new Set(paths).size, names.length);
  for (const path of paths) {
    assert.equal(dirname(path), scratch);
    assert.ok(lstatSync(path).isDirectory());
  }
  assert.deepEqual(paths.map((path) => path.slice(scratch.length + 1).replace(/\.[^.]+$/, '')),
    ['locus-e2e', 'locus-ws', 'locus-security-e2e', 'locus-security-ws', 'locus-graph-imp']);
});

// Keep the original whole-script coupling: the graph rehearsal must succeed
// before a later fresh-quickstart failure, and the outer trap must clean up.
test('the graph home spaced-parent probe succeeds before fresh quickstart fails and cleans up', (t) => {
  const fixture = setup(t);
  const scratch = join(fixture.root, 'outer temp');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  const result = run(fixture, scratch);
  assert.equal(result.signal, null);
  assert.equal(result.status, 1, 'the fixture stops later at fresh quickstart');
  assert.match(result.stdout, /ok  GRAPH_HOME creation succeeds beneath a TMPDIR containing spaces/);
  assert.match(result.stderr, /fresh quickstart failed/);
  assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'unrelated');
  assert.deepEqual(readdirSync(scratch), ['keep']);
});

test('the graph home spaced-parent probe rejects a misplaced directory in the whole script', (t) => {
  const fixture = setup(t);
  const scratch = join(fixture.root, 'outer temp');
  const wrongRoot = join(fixture.root, 'wrong root');
  mkdirSync(scratch);
  mkdirSync(wrongRoot);
  const realMktemp = execFileSync('which', ['mktemp'], { encoding: 'utf8' }).trim();
  writeFileSync(join(fixture.bin, 'mktemp'), `#!/bin/sh
case "$2" in
  */locus-graph-imp.XXXXXX) exec "$REAL_MKTEMP" -d "$WRONG_ROOT/locus-graph-imp.XXXXXX" ;;
  *) exec "$REAL_MKTEMP" "$@" ;;
esac
`, { mode: 0o755 });
  const result = run(fixture, scratch, { REAL_MKTEMP: realMktemp, WRONG_ROOT: wrongRoot });
  assert.equal(result.signal, null);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /GRAPH_HOME was not created beneath the space-containing TMPDIR/);
  assert.doesNotMatch(result.stdout, /ok  GRAPH_HOME creation succeeds/);
  assert.doesNotMatch(result.stderr, /fresh quickstart failed/);
  assert.deepEqual(readdirSync(wrongRoot), [], 'exit cleanup removes the misplaced home');
  assert.deepEqual(readdirSync(scratch), []);
});
