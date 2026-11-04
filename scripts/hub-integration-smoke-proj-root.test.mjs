// Run: node --test scripts/hub-integration-smoke-proj-root.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
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
