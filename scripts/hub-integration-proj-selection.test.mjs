// Run: node --test scripts/hub-integration-proj-selection.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import {
  existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, realpathSync,
  rmSync, symlinkSync, writeFileSync,
} from 'node:fs';
import { basename, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-integration-test.sh');

// Stub the prerequisites and exercise only SMOKE_PROJ root selection. The
// wrapped mktemp records both the template it was handed and the directory it
// created. The locus stub checks SMOKE_PROJ at init — PARENT is the parent
// string the test expects ${SMOKE_PROJ%/*} to equal verbatim — then either
// stops the run there (default) or lets it continue to the first post-cd
// `locus status`, which must run inside the project with .mcp.json already
// written. Either way the injected nonzero return triggers the EXIT trap.
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
  case "${'$'}{1:-}" in
    init)
      [[ -d "$SMOKE_HOME" && -d "$SMOKE_PROJ" && "$SMOKE_HOME" != "$SMOKE_PROJ" ]] || return 74
      [[ "${'$'}{SMOKE_PROJ%/*}" == "$PARENT" ]] || return 75
      if [[ -n "${'$'}{MODE:-}" ]]; then
        stat -f %Lp "$SMOKE_PROJ" 2>/dev/null > "$MODE" || stat -c %a "$SMOKE_PROJ" > "$MODE" || return 76
      fi
      printf '%s\\n' "$SMOKE_HOME" "$SMOKE_PROJ" > "$OBSERVED"
      [[ "${'$'}{STOP_AT:-init}" == init ]] || return 0
      return 73
      ;;
    status)
      [[ -f .mcp.json ]] || return 77
      printf '%s\\n' "$PWD" > "$USED"
      return 73
      ;;
  esac
}
export -f node jq cargo mktemp locus
exec bash "$1"
`;

// Acceptance: a temporary TMPDIR contains the project directory — the run
// creates SMOKE_PROJ inside the supplied root, works in it, and cleans it.
test('SMOKE_PROJ is contained by a temporary TMPDIR and used as the working directory', (t) => {
  const scratch = realpathSync(mkdtempSync('/tmp/locus-proj-sel-'));
  t.after(() => rmSync(scratch, { recursive: true, force: true }));
  const tmpdir = join(scratch, 'isolated root');
  const templates = join(scratch, 'templates');
  const allocated = join(scratch, 'allocated');
  const observed = join(scratch, 'observed');
  const used = join(scratch, 'used');
  mkdirSync(tmpdir);
  writeFileSync(join(tmpdir, 'keep'), 'sentinel');

  const result = spawnSync('bash', ['-c', harness, 'proj-selection', script], {
    env: { PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: tmpdir, PARENT: tmpdir,
      TEMPLATES: templates, ALLOCATED: allocated, OBSERVED: observed, USED: used, STOP_AT: 'status' },
    encoding: 'utf8',
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 73, 'status must observe the project live beneath TMPDIR, then stop');

  const roots = readFileSync(templates, 'utf8').trim().split('\n');
  assert.equal(roots.length, 2, 'exactly two temporary directories are allocated before init');
  assert.equal(roots[1], `${tmpdir}/locus-hub-int-proj.XXXXXX`,
    'the project template must be rooted at the supplied TMPDIR');

  const paths = readFileSync(allocated, 'utf8').trim().split('\n');
  const [home, proj] = readFileSync(observed, 'utf8').trim().split('\n');
  assert.equal(proj, paths[1], 'SMOKE_PROJ must be the second allocation, not the home');
  assert.equal(dirname(proj), tmpdir, 'the project must be created directly beneath TMPDIR');
  assert.match(basename(proj), /^locus-hub-int-proj\.[^/]+$/);
  assert.equal(readFileSync(used, 'utf8').trim(), proj,
    'checks must run inside the directory mktemp created, reached through TMPDIR');
  for (const path of paths) {
    assert.equal(existsSync(path), false, 'each allocated directory must be removed on exit');
  }
  assert.equal(home, paths[0]);
  assert.deepEqual(readdirSync(tmpdir), ['keep'], 'cleanup must leave the supplied root untouched');
  assert.equal(readFileSync(join(tmpdir, 'keep'), 'utf8'), 'sentinel');
});

// A symlinked TMPDIR is a legitimate temporary root: the project template must
// carry the link path verbatim (never canonicalized), the directory must
// materialize in the target, and cleanup must remove it through the link while
// the link itself survives.
test('SMOKE_PROJ uses a symlinked TMPDIR verbatim', (t) => {
  const scratch = realpathSync(mkdtempSync('/tmp/locus-proj-sel-'));
  t.after(() => rmSync(scratch, { recursive: true, force: true }));
  const target = join(scratch, 'real root');
  const link = join(scratch, 'linked root');
  const templates = join(scratch, 'templates');
  const allocated = join(scratch, 'allocated');
  const observed = join(scratch, 'observed');
  const used = join(scratch, 'used');
  mkdirSync(target);
  writeFileSync(join(target, 'keep'), 'sentinel');
  symlinkSync(target, link, 'dir');

  const result = spawnSync('bash', ['-c', harness, 'proj-symlink', script], {
    env: { PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: link, PARENT: link,
      TEMPLATES: templates, ALLOCATED: allocated, OBSERVED: observed, USED: used, STOP_AT: 'status' },
    encoding: 'utf8',
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 73, 'status must observe the project through the link, then stop');

  const roots = readFileSync(templates, 'utf8').trim().split('\n');
  assert.equal(roots[1], `${link}/locus-hub-int-proj.XXXXXX`,
    'the project template must carry the link path, not the resolved target');
  const proj = readFileSync(observed, 'utf8').trim().split('\n')[1];
  assert.equal(proj, readFileSync(allocated, 'utf8').trim().split('\n')[1]);
  assert.equal(dirname(proj), link, 'the link path is used verbatim, not canonicalized');
  assert.equal(realpathSync(dirname(proj)), realpathSync(target),
    'the project must materialize inside the link target');
  assert.equal(readFileSync(used, 'utf8').trim(), proj, 'checks run inside the project through the link');
  assert.equal(existsSync(proj), false, 'the project must be removed through the link');
  assert.ok(lstatSync(link).isSymbolicLink(), 'the TMPDIR symlink itself must survive');
  assert.deepEqual(readdirSync(target), ['keep'], 'materialized in — and removed from — the target');
});

// Only TMPDIR selects the project root: a bare `mktemp template` would create
// in the caller's cwd, and a root derived from HOME would land there instead.
// Both decoys must stay untouched.
test('SMOKE_PROJ lands beneath the supplied TMPDIR, not ambient roots', (t) => {
  const scratch = realpathSync(mkdtempSync('/tmp/locus-proj-sel-'));
  t.after(() => rmSync(scratch, { recursive: true, force: true }));
  const controlled = join(scratch, 'controlled');
  const decoys = [join(scratch, 'decoy cwd'), join(scratch, 'decoy home')];
  const templates = join(scratch, 'templates');
  const allocated = join(scratch, 'allocated');
  const observed = join(scratch, 'observed');
  const used = join(scratch, 'used');
  for (const dir of [controlled, ...decoys]) {
    mkdirSync(dir);
    writeFileSync(join(dir, 'keep'), 'sentinel');
  }

  const result = spawnSync('bash', ['-c', harness, 'proj-decoys', script], {
    cwd: decoys[0],
    env: { PATH: process.env.PATH, HOME: decoys[1], TMPDIR: controlled, PARENT: controlled,
      TEMPLATES: templates, ALLOCATED: allocated, OBSERVED: observed, USED: used, STOP_AT: 'status' },
    encoding: 'utf8',
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 73, 'status must observe the project beneath the controlled root');

  const roots = readFileSync(templates, 'utf8').trim().split('\n');
  assert.equal(roots[1], `${controlled}/locus-hub-int-proj.XXXXXX`);
  const proj = readFileSync(observed, 'utf8').trim().split('\n')[1];
  assert.equal(dirname(proj), controlled);
  assert.equal(readFileSync(used, 'utf8').trim(), proj);
  assert.equal(existsSync(proj), false);
  assert.deepEqual(readdirSync(controlled), ['keep'], 'the selected root keeps only its own entries');
  for (const decoy of decoys) {
    assert.deepEqual(readdirSync(decoy), ['keep'], `ambient root ${decoy} must gain no project directory`);
    assert.equal(readFileSync(join(decoy, 'keep'), 'utf8'), 'sentinel');
  }
});

// Only relative TMPDIR values are normalized; an absolute path containing `..`
// segments is handed to mktemp verbatim. The project must still resolve inside
// the real temporary root and be cleaned from it.
test('SMOKE_PROJ resolves beneath a TMPDIR containing .. segments', (t) => {
  const scratch = realpathSync(mkdtempSync('/tmp/locus-proj-sel-'));
  t.after(() => rmSync(scratch, { recursive: true, force: true }));
  const seg = join(scratch, 'seg');
  const root = join(scratch, 'root dir');
  const templates = join(scratch, 'templates');
  const allocated = join(scratch, 'allocated');
  const observed = join(scratch, 'observed');
  mkdirSync(seg);
  mkdirSync(root);
  writeFileSync(join(root, 'keep'), 'sentinel');
  const tmpdir = `${seg}/../${basename(root)}`;

  const result = spawnSync('bash', ['-c', harness, 'proj-dotdot', script], {
    env: { PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: tmpdir, PARENT: tmpdir,
      TEMPLATES: templates, ALLOCATED: allocated, OBSERVED: observed },
    encoding: 'utf8',
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 73, 'init must observe the project beneath the .. root, then stop');

  const roots = readFileSync(templates, 'utf8').trim().split('\n');
  assert.equal(roots[1], `${tmpdir}/locus-hub-int-proj.XXXXXX`,
    'the project template must carry the supplied path verbatim');
  const proj = readFileSync(observed, 'utf8').trim().split('\n')[1];
  assert.ok(proj.startsWith(`${tmpdir}/`), 'the recorded project must use the supplied path verbatim');
  assert.equal(realpathSync(dirname(proj)), realpathSync(root),
    'the project must resolve inside the real temporary root');
  assert.equal(existsSync(proj), false, 'the project must be removed on exit');
  assert.deepEqual(readdirSync(root), ['keep']);
  assert.deepEqual(readdirSync(seg), [], 'no directory may be created inside the traversed segment');
});

// The project must be a real mktemp -d directory — private to the user — not a
// plain mkdir beneath TMPDIR.
test('SMOKE_PROJ is private (0700) beneath the supplied TMPDIR', (t) => {
  const scratch = realpathSync(mkdtempSync('/tmp/locus-proj-sel-'));
  t.after(() => rmSync(scratch, { recursive: true, force: true }));
  const tmpdir = join(scratch, 'perm root');
  const templates = join(scratch, 'templates');
  const allocated = join(scratch, 'allocated');
  const observed = join(scratch, 'observed');
  const mode = join(scratch, 'mode');
  mkdirSync(tmpdir);

  const result = spawnSync('bash', ['-c', harness, 'proj-mode', script], {
    env: { PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: tmpdir, PARENT: tmpdir,
      TEMPLATES: templates, ALLOCATED: allocated, OBSERVED: observed, MODE: mode },
    encoding: 'utf8',
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 73, 'init must observe the live project before cleanup');
  const proj = readFileSync(observed, 'utf8').trim().split('\n')[1];
  assert.equal(dirname(proj), tmpdir, 'the project must be created directly beneath the exact TMPDIR');
  assert.match(basename(proj), /^locus-hub-int-proj\.[^/]+$/);
  assert.equal(readFileSync(mode, 'utf8').trim(), '700', 'mktemp -d privacy mode must be preserved');
  assert.equal(existsSync(proj), false, 'the project must be removed on exit');
});
