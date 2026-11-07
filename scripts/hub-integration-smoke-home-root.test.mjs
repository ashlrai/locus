// Run: node --test scripts/hub-integration-smoke-home-root.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { basename, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-integration-test.sh');

// Stub the prerequisites and stop at `locus init`, after confirming the
// exported LOCUS_HOME is a real directory directly beneath the isolated
// TMPDIR. The wrapped mktemp records both the template it was handed and the
// directory it created, so the test proves the supplied TMPDIR is what
// selects the home's parent. The injected init failure triggers the EXIT
// trap and its cleanup.
const harness = `
node() { :; }
jq() { :; }
cargo() { :; }
mktemp() {
  local template="\${!#}" path
  printf '%s\\n' "$template" >> "$TEMPLATES"
  path="$(command mktemp "$@")" || return
  printf '%s\\n' "$path" >> "$RECORD"
  printf '%s\\n' "$path"
}
locus() {
  if [[ "$1" == init ]]; then
    [[ -d "$LOCUS_HOME" && "${'$'}{LOCUS_HOME%/*}" == "$TMPDIR" ]] || return 74
    printf '%s\\n' "$LOCUS_HOME" > "$OBSERVED"
    return 73
  fi
}
export -f node jq cargo mktemp locus
exec bash "$1"
`;

test('SMOKE_HOME is created directly beneath an isolated TMPDIR', (t) => {
  const scratch = mkdtempSync('/tmp/locus-home-root-');
  t.after(() => rmSync(scratch, { recursive: true, force: true }));
  const tmpdir = join(scratch, 'isolated root');
  const templates = join(scratch, 'templates');
  const record = join(scratch, 'allocated');
  const observed = join(scratch, 'observed');
  mkdirSync(tmpdir);
  writeFileSync(join(tmpdir, 'keep'), 'sentinel');

  const result = spawnSync('bash', ['-c', harness, 'smoke-home-root', script], {
    env: { PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: tmpdir,
      TEMPLATES: templates, RECORD: record, OBSERVED: observed },
    encoding: 'utf8',
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 73, 'the injected init stop must propagate after observing SMOKE_HOME');

  const roots = readFileSync(templates, 'utf8').trim().split('\n');
  assert.equal(roots.length, 2, 'exactly two temporary directories are allocated before init');
  assert.equal(roots[0], `${tmpdir}/locus-hub-int.XXXXXX`,
    'the SMOKE_HOME template must be rooted at the supplied TMPDIR');
  const allocated = readFileSync(record, 'utf8').trim().split('\n');
  assert.equal(allocated.length, 2, 'both temporary directories must be allocated before init');
  const home = readFileSync(observed, 'utf8').trim();
  assert.equal(home, allocated[0], 'LOCUS_HOME must be the first allocation, SMOKE_HOME');
  assert.equal(dirname(home), tmpdir, 'SMOKE_HOME must be created directly beneath TMPDIR');
  assert.match(basename(home), /^locus-hub-int\.[^/]+$/,
    'SMOKE_HOME must use the locus-hub-int.XXXXXX template');
  assert.equal(dirname(allocated[1]), tmpdir,
    'the smoke project must share the same temporary root');
  assert.match(basename(allocated[1]), /^locus-hub-int-proj\.[^/]+$/);
  assert.ok(result.stdout.includes(`== hub-integration-test LOCUS_HOME=${home} ==`),
    'the smoke run must report the home created beneath TMPDIR');
  for (const path of allocated) {
    assert.equal(existsSync(path), false, 'each allocated directory must be removed on exit');
  }
  assert.deepEqual(readdirSync(tmpdir), ['keep'], 'cleanup must leave the isolated root untouched');
  assert.equal(readFileSync(join(tmpdir, 'keep'), 'utf8'), 'sentinel');
});

test('a relative TMPDIR selects the home root and is safe to clean up', (t) => {
  const scratch = realpathSync(mkdtempSync('/tmp/locus-home-relative-'));
  t.after(() => rmSync(scratch, { recursive: true, force: true }));
  const relativeRoot = 'root [*] with spaces';
  const tmpdir = join(scratch, relativeRoot);
  const templates = join(scratch, 'templates');
  const record = join(scratch, 'allocated');
  const observed = join(scratch, 'observed');
  mkdirSync(tmpdir);
  writeFileSync(join(tmpdir, 'keep'), 'sentinel');

  const result = spawnSync('bash', ['-c', harness, 'smoke-home-relative', script], {
    cwd: scratch,
    env: { PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: relativeRoot,
      TEMPLATES: templates, RECORD: record, OBSERVED: observed },
    encoding: 'utf8',
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 73, 'init must observe a live home under the normalized root');
  const roots = readFileSync(templates, 'utf8').trim().split('\n');
  assert.equal(roots.length, 2, 'exactly two temporary directories are allocated before init');
  assert.equal(roots[0], `${tmpdir}/locus-hub-int.XXXXXX`,
    'the relative TMPDIR must be normalized to an absolute root before allocation');
  const allocated = readFileSync(record, 'utf8').trim().split('\n');
  assert.equal(allocated.length, 2);
  const home = readFileSync(observed, 'utf8').trim();
  assert.equal(home, allocated[0]);
  assert.equal(dirname(home), tmpdir);
  assert.match(basename(home), /^locus-hub-int\.[^/]+$/);
  for (const path of allocated) {
    assert.equal(dirname(path), tmpdir);
    assert.equal(existsSync(path), false, 'cleanup must remove the allocated directory');
  }
  assert.deepEqual(readdirSync(tmpdir), ['keep']);
  assert.equal(readFileSync(join(tmpdir, 'keep'), 'utf8'), 'sentinel');
});

// Selection only holds while SMOKE_HOME is allocated exactly once through the
// shared helper; a later assignment could silently re-root the home.
test('SMOKE_HOME is only ever assigned through make_smoke_dir', () => {
  const source = readFileSync(script, 'utf8');
  const assigns = source.split('\n').filter((line) => /^\s*SMOKE_HOME=/.test(line));
  const allocations = assigns.filter((line) => !/SMOKE_HOME=""/.test(line));
  assert.equal(allocations.length, 1, `unexpected SMOKE_HOME allocations: ${allocations.join(' | ')}`);
  assert.match(allocations[0], /SMOKE_HOME="\$\(make_smoke_dir locus-hub-int\)"/,
    'the sole allocation must go through the shared helper');
  const helper = source.match(/^make_smoke_dir\(\) \{\n([\s\S]*?)^\}/m);
  assert.ok(helper, 'hub-integration-test.sh must define make_smoke_dir');
  assert.match(helper[1], /mktemp -d "\$\{TMPDIR:-\/tmp\}\/\$1\.XXXXXX"/,
    'the helper must allocate beneath ${TMPDIR:-/tmp}');
});

// Shell metacharacters in TMPDIR are literal path characters: the template
// must reach mktemp verbatim, nothing may be evaluated, and the created
// directory must still sit directly beneath the requested root.
test('SMOKE_HOME treats shell metacharacters in TMPDIR as literal path characters', (t) => {
  const scratch = mkdtempSync('/tmp/locus-home-root-');
  t.after(() => rmSync(scratch, { recursive: true, force: true }));
  const tmpdir = join(scratch, 'root [*] $(touch injected)');
  const templates = join(scratch, 'templates');
  const record = join(scratch, 'allocated');
  const observed = join(scratch, 'observed');
  mkdirSync(tmpdir);
  writeFileSync(join(tmpdir, 'keep'), 'sentinel');

  const result = spawnSync('bash', ['-c', harness, 'smoke-home-literal', script], {
    cwd: scratch,
    env: { PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: tmpdir,
      TEMPLATES: templates, RECORD: record, OBSERVED: observed },
    encoding: 'utf8',
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 73, 'init must observe the live home before cleanup');
  const roots = readFileSync(templates, 'utf8').trim().split('\n');
  assert.equal(roots.length, 2, 'exactly two temporary directories are allocated before init');
  assert.equal(roots[0], `${tmpdir}/locus-hub-int.XXXXXX`,
    'the SMOKE_HOME template must keep the metacharacters verbatim');
  const home = readFileSync(observed, 'utf8').trim();
  assert.equal(dirname(home), tmpdir);
  assert.match(basename(home), /^locus-hub-int\.[^/]+$/);
  assert.equal(existsSync(home), false, 'SMOKE_HOME must be removed on exit');
  assert.equal(existsSync(join(scratch, 'injected')), false, 'path characters must not be evaluated');
  assert.deepEqual(readdirSync(tmpdir), ['keep']);
  assert.equal(readFileSync(join(tmpdir, 'keep'), 'utf8'), 'sentinel');
});

// macOS exports TMPDIR with a trailing slash, so the mktemp template gains a
// doubled separator. The home must still be allocated directly beneath the
// supplied root — not beside it or under /tmp — and cleanup must remove it.
test('SMOKE_HOME is created beneath a TMPDIR supplied with a trailing slash', (t) => {
  const scratch = mkdtempSync('/tmp/locus-home-root-');
  t.after(() => rmSync(scratch, { recursive: true, force: true }));
  const tmpdir = join(scratch, 'trailing root');
  const templates = join(scratch, 'templates');
  const record = join(scratch, 'allocated');
  const observed = join(scratch, 'observed');
  mkdirSync(tmpdir);
  writeFileSync(join(tmpdir, 'keep'), 'sentinel');

  const result = spawnSync('bash', ['-c', harness, 'smoke-home-trailing', script], {
    env: { PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: `${tmpdir}/`,
      TEMPLATES: templates, RECORD: record, OBSERVED: observed },
    encoding: 'utf8',
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 73, 'init must observe SMOKE_HOME live beneath the supplied root, then stop');

  const roots = readFileSync(templates, 'utf8').trim().split('\n');
  assert.equal(roots.length, 2, 'exactly two temporary directories are allocated before init');
  assert.equal(roots[0], `${tmpdir}//locus-hub-int.XXXXXX`,
    'the SMOKE_HOME template must still be rooted at the supplied TMPDIR');
  const home = readFileSync(observed, 'utf8').trim();
  assert.equal(realpathSync(dirname(home)), realpathSync(tmpdir),
    'SMOKE_HOME must resolve to a directory directly beneath the supplied root');
  assert.match(basename(home), /^locus-hub-int\.[^/]+$/);
  assert.equal(existsSync(home), false, 'SMOKE_HOME must be removed on exit');
  assert.deepEqual(readdirSync(tmpdir), ['keep'], 'cleanup must leave the supplied root untouched');
  assert.equal(readFileSync(join(tmpdir, 'keep'), 'utf8'), 'sentinel');
});

// With TMPDIR unset the script must fall back to /tmp, and SMOKE_HOME must be
// a live directory there while the run is in progress (not just a recorded path).
test('SMOKE_HOME is a live directory under /tmp when TMPDIR is unset', (t) => {
  const scratch = mkdtempSync('/tmp/locus-home-root-');
  t.after(() => rmSync(scratch, { recursive: true, force: true }));
  const templates = join(scratch, 'templates');
  const record = join(scratch, 'allocated');
  const observed = join(scratch, 'observed');
  const unsetHarness = harness.replace('"${LOCUS_HOME%/*}" == "$TMPDIR"', '"${LOCUS_HOME%/*}" == /tmp');
  assert.notEqual(unsetHarness, harness, 'the parent-directory check must be rewritten to expect /tmp');

  const result = spawnSync('bash', ['-c', unsetHarness, 'smoke-home-default', script], {
    env: { PATH: process.env.PATH, HOME: process.env.HOME, TEMPLATES: templates,
      RECORD: record, OBSERVED: observed },
    encoding: 'utf8',
  });

  assert.equal(result.error, undefined);
  assert.equal(result.status, 73, 'init must observe SMOKE_HOME as an existing directory directly under /tmp');
  const roots = readFileSync(templates, 'utf8').trim().split('\n');
  assert.equal(roots.length, 2, 'exactly two temporary directories are allocated before init');
  assert.equal(roots[0], '/tmp/locus-hub-int.XXXXXX',
    'the SMOKE_HOME template must fall back to /tmp without TMPDIR');
  const home = readFileSync(observed, 'utf8').trim();
  assert.equal(dirname(home), '/tmp');
  assert.match(basename(home), /^locus-hub-int\.[^/]+$/);
  assert.equal(existsSync(home), false, 'SMOKE_HOME must be removed on exit');
});
