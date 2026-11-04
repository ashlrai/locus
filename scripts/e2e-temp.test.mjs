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
  // Avoid building binaries and fail immediately after the cleanup trap is set.
  writeFileSync(join(bin, 'cargo'), '#!/bin/sh\nexit 0\n', { mode: 0o755 });
  const locus = join(bin, 'locus');
  writeFileSync(locus, '#!/bin/sh\nexit 1\n', { mode: 0o755 });
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
  const start = source.indexOf('export LOCUS_HOME\n');
  const trap = "trap 'e2e_cleanup' EXIT";
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
  const start = source.indexOf('export LOCUS_HOME\n');
  const trap = "trap 'e2e_cleanup' EXIT";
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
    printf '%s' "$home" > "$HOME_CREATED"
    touch "$home/sentinel"
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
  const start = source.indexOf('export LOCUS_HOME\n');
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
  const createGraphHome = source.match(/^\s*GRAPH_HOME="\$\(mktemp -d .*\)"$/m)?.[0];
  assert.ok(cleanup, 'missing e2e cleanup function');
  assert.ok(createGraphHome, 'missing graph home creation');

  const result = spawnSync('bash', ['-c', `set -euo pipefail
${cleanup}
LOCUS_HOME="$1/main-home"
mkdir "$LOCUS_HOME"
WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
trap 'e2e_cleanup' EXIT
${createGraphHome}
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
    // Redirect only the main home; security probes must use their own TMPDIR.
    writeFileSync(join(fixture.bin, 'mktemp'), `#!/bin/sh
case "$2" in
  */locus-e2e.XXXXXX)
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

test('workspace cleanup survives a command failure after leaving the workspace', (t) => {
  const fixture = setup(t);
  const scratch = join(fixture.root, 'custom tmp $literal; [test]');
  const record = join(fixture.root, 'workspace-path');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  const source = readFileSync(script, 'utf8');
  const cleanup = source.match(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m)?.[0];
  const start = source.indexOf('WS_DIR="$(mktemp -d "${TMPDIR:-/tmp}/locus-ws.XXXXXX")"');
  const end = source.indexOf('\n# leave force-pin;', start);
  assert.ok(cleanup && start >= 0 && end > start, 'missing workspace lifecycle');
  const workspaceSection = source.slice(start, end);
  const probe = `set -euo pipefail
${cleanup}
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
    const source = readFileSync(script, 'utf8');
    const cleanup = source.match(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m)?.[0];
    const createWorkspace = source.match(/^WS_DIR="\$\(mktemp -d .*\)"$/m)?.[0];
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
    assert.deepEqual(readdirSync(scratch), ['keep'], 'cleanup must remove workspace and preserve unrelated files');
  });
}

test('repeated workspace creation yields distinct directories beneath TMPDIR', (t) => {
  const fixture = setup(t);
  const scratch = join(fixture.root, 'custom tmp');
  const observed = join(fixture.root, 'observed-workspaces');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');
  const source = readFileSync(script, 'utf8');
  const cleanup = source.match(/^e2e_cleanup\(\) \{[\s\S]*?^\}/m)?.[0];
  const createWorkspace = source.match(/^WS_DIR="\$\(mktemp -d .*\)"$/m)?.[0];
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
  const createWorkspace = source.match(/^WS_DIR="\$\(mktemp -d .*\)"$/m)?.[0];
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

// Stubbed mktemp records the template the script's real WS_DIR line asks for,
// so nothing is created in the real /tmp for the fallback cases.
for (const [label, temp] of [['custom', 'custom tmp $literal; [test]'], ['unset', undefined], ['empty', '']]) {
  test(`workspace template uses ${label} TMPDIR root`, (t) => {
    const fixture = setup(t);
    const scratch = join(fixture.root, temp === undefined || temp === '' ? 'unused' : temp);
    const capture = join(fixture.root, 'template');
    mkdirSync(scratch);
    const createWorkspace = readFileSync(script, 'utf8').match(/^WS_DIR="\$\(mktemp -d .*\)"$/m)?.[0];
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
