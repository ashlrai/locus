// Run: node --test scripts/hub-integration-smoke-proj-root.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { basename, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-integration-test.sh');

// Stub the prerequisites and stop at `locus init`, after confirming SMOKE_PROJ
// is a real directory directly beneath the expected root (PARENT). The wrapped
// mktemp records both the template it was handed and the directory it created,
// so the test proves the supplied TMPDIR is what selects the project's parent.
// The injected init failure triggers the EXIT trap and its cleanup.
const harness = `
node() { :; }
jq() { :; }
cargo() { :; }
mktemp() {
  local template="\${!#}" path
  printf '%s\\n' "$template" >> "$TEMPLATES"
  path="$(command mktemp "$@")" || return
  printf '%s\\n' "$path" >> "$ALLOCATED"
  printf '%s\\n' "$path"
}
locus() {
  if [[ "$1" == init ]]; then
    [[ -d "$SMOKE_HOME" && -d "$SMOKE_PROJ" && "$SMOKE_HOME" != "$SMOKE_PROJ" ]] || return 74
    [[ "${'$'}{SMOKE_PROJ%/*}" == "$PARENT" ]] || return 75
    printf '%s\\n' "$SMOKE_HOME" "$SMOKE_PROJ" > "$OBSERVED"
    return 73
  fi
}
export -f node jq cargo mktemp locus
exec bash "$1"
`;

test('SMOKE_PROJ is created directly beneath the supplied TMPDIR', (t) => {
  const scratch = mkdtempSync('/tmp/locus-proj-root-');
  t.after(() => rmSync(scratch, { recursive: true, force: true }));
  const tmpdir = join(scratch, 'isolated root');
  const templates = join(scratch, 'templates');
  const allocated = join(scratch, 'allocated');
  const observed = join(scratch, 'observed');
  mkdirSync(tmpdir);
  writeFileSync(join(tmpdir, 'keep'), 'sentinel');

  const result = spawnSync('bash', ['-c', harness, 'smoke-proj-root', script], {
    env: { PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: tmpdir, PARENT: tmpdir,
      TEMPLATES: templates, ALLOCATED: allocated, OBSERVED: observed },
    encoding: 'utf8',
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 73, 'init must observe SMOKE_PROJ live beneath TMPDIR, then stop');

  const roots = readFileSync(templates, 'utf8').trim().split('\n');
  assert.equal(roots.length, 2, 'exactly two temporary directories are allocated before init');
  assert.equal(roots[1], `${tmpdir}/locus-hub-int-proj.XXXXXX`,
    'the project template must be rooted at the supplied TMPDIR');

  const paths = readFileSync(allocated, 'utf8').trim().split('\n');
  const [home, proj] = readFileSync(observed, 'utf8').trim().split('\n');
  assert.equal(proj, paths[1], 'SMOKE_PROJ must be the second allocation, not the home');
  assert.equal(home, paths[0], 'SMOKE_HOME must be the first allocation');
  assert.equal(dirname(proj), tmpdir, 'SMOKE_PROJ must be created directly beneath TMPDIR');
  assert.match(basename(proj), /^locus-hub-int-proj\.[^/]+$/,
    'SMOKE_PROJ must use the locus-hub-int-proj.XXXXXX template');
  assert.equal(dirname(home), tmpdir, 'SMOKE_HOME must share the same temporary root');
  assert.notEqual(proj, home);
  for (const path of paths) {
    assert.equal(existsSync(path), false, 'each allocated directory must be removed on exit');
  }
  assert.deepEqual(readdirSync(tmpdir), ['keep'], 'cleanup must leave the supplied root untouched');
  assert.equal(readFileSync(join(tmpdir, 'keep'), 'utf8'), 'sentinel');
});

// macOS exports TMPDIR with a trailing slash, so the mktemp template gains a
// doubled separator. The project must still be allocated directly beneath the
// supplied root — not beside it or under /tmp — and cleanup must remove it.
test('SMOKE_PROJ is created beneath a TMPDIR supplied with a trailing slash', (t) => {
  const scratch = mkdtempSync('/tmp/locus-proj-root-');
  t.after(() => rmSync(scratch, { recursive: true, force: true }));
  const tmpdir = join(scratch, 'trailing root');
  const templates = join(scratch, 'templates');
  const allocated = join(scratch, 'allocated');
  const observed = join(scratch, 'observed');
  mkdirSync(tmpdir);
  writeFileSync(join(tmpdir, 'keep'), 'sentinel');

  // ${SMOKE_PROJ%/*} keeps the doubled separator, so PARENT carries it too.
  const result = spawnSync('bash', ['-c', harness, 'smoke-proj-trailing', script], {
    env: { PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: `${tmpdir}/`, PARENT: `${tmpdir}/`,
      TEMPLATES: templates, ALLOCATED: allocated, OBSERVED: observed },
    encoding: 'utf8',
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 73, 'init must observe SMOKE_PROJ live beneath the supplied root, then stop');

  const roots = readFileSync(templates, 'utf8').trim().split('\n');
  assert.equal(roots.length, 2, 'exactly two temporary directories are allocated before init');
  assert.equal(roots[1], `${tmpdir}//locus-hub-int-proj.XXXXXX`,
    'the project template must still be rooted at the supplied TMPDIR');

  const proj = readFileSync(observed, 'utf8').trim().split('\n')[1];
  assert.equal(realpathSync(dirname(proj)), realpathSync(tmpdir),
    'SMOKE_PROJ must resolve to a directory directly beneath the supplied root');
  assert.match(basename(proj), /^locus-hub-int-proj\.[^/]+$/);
  assert.equal(existsSync(proj), false, 'SMOKE_PROJ must be removed on exit');
  assert.deepEqual(readdirSync(tmpdir), ['keep'], 'cleanup must leave the supplied root untouched');
  assert.equal(readFileSync(join(tmpdir, 'keep'), 'utf8'), 'sentinel');
});

// Selection only holds while SMOKE_PROJ is allocated exactly once through the
// shared helper; a later assignment could silently re-root the project.
test('SMOKE_PROJ is only ever assigned through make_smoke_dir', () => {
  const source = readFileSync(script, 'utf8');
  const assigns = source.split('\n').filter((line) => /^\s*SMOKE_PROJ=/.test(line));
  const allocations = assigns.filter((line) => !/SMOKE_PROJ=""/.test(line));
  assert.equal(allocations.length, 1, `unexpected SMOKE_PROJ allocations: ${allocations.join(' | ')}`);
  assert.match(allocations[0], /SMOKE_PROJ="\$\(make_smoke_dir locus-hub-int-proj\)"/,
    'the sole allocation must go through the shared helper');
  const helper = source.match(/^make_smoke_dir\(\) \{\n([\s\S]*?)^\}/m);
  assert.ok(helper, 'hub-integration-test.sh must define make_smoke_dir');
  assert.match(helper[1], /mktemp -d "\$\{TMPDIR:-\/tmp\}\/\$1\.XXXXXX"/,
    'the helper must allocate beneath ${TMPDIR:-/tmp}');
});

// With TMPDIR unset the script must fall back to /tmp, and SMOKE_PROJ must be
// a live directory there while the run is in progress (not just a recorded path).
test('SMOKE_PROJ is a live directory under /tmp when TMPDIR is unset', (t) => {
  const scratch = mkdtempSync('/tmp/locus-proj-root-');
  t.after(() => rmSync(scratch, { recursive: true, force: true }));
  const templates = join(scratch, 'templates');
  const allocated = join(scratch, 'allocated');
  const observed = join(scratch, 'observed');

  const result = spawnSync('bash', ['-c', harness, 'smoke-proj-default', script], {
    env: { PATH: process.env.PATH, HOME: process.env.HOME, PARENT: '/tmp',
      TEMPLATES: templates, ALLOCATED: allocated, OBSERVED: observed },
    encoding: 'utf8',
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 73, 'init must observe SMOKE_PROJ as an existing directory under /tmp');

  const roots = readFileSync(templates, 'utf8').trim().split('\n');
  assert.equal(roots.length, 2, 'exactly two temporary directories are allocated before init');
  assert.equal(roots[1], '/tmp/locus-hub-int-proj.XXXXXX',
    'the project template must fall back to /tmp without TMPDIR');
  const proj = readFileSync(observed, 'utf8').trim().split('\n')[1];
  assert.equal(dirname(proj), '/tmp');
  assert.match(basename(proj), /^locus-hub-int-proj\.[^/]+$/);
  assert.equal(existsSync(proj), false, 'SMOKE_PROJ must be removed on exit');
});
