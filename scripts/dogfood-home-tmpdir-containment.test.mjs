// Run: node --test scripts/dogfood-home-tmpdir-containment.test.mjs
// The isolated dogfood home is allocated before any probe runs: a supplied,
// nonempty TMPDIR is the containment boundary the caller chose, and the
// allocated DOGFOOD_HOME must live inside it as a real directory entry.
// Selection must not drift to other temp variables, an inherited DOGFOOD_HOME
// or the working directory, and must fail closed — never fall back to another
// root — when the supplied TMPDIR does not exist. The probes below run the
// script's real mainline; fixture stubs provide only cargo, locus-mcp and a
// recording locus (plus, where noted, a recording mktemp).
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, lstatSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'dogfood.sh');

// dogfood.sh resolves its repo root from its own path and prepends
// $ROOT/target/debug to PATH, so a fixture checkout — the copied script plus
// stub binaries — runs the real mainline with every external command stubbed.
// The locus stub runs the in-script containment assertions during `init` (the
// first command after the root is selected, with the run already cd'd into
// $LOCUS_HOME/project), records the selected home, then stops the run at a
// controlled quickstart failure so no probe ever runs.
function fixture(root, locusStub, extraStubs = {}) {
  const fixtureRoot = join(root, 'fixture');
  const bin = join(fixtureRoot, 'target', 'debug');
  mkdirSync(bin, { recursive: true });
  mkdirSync(join(fixtureRoot, 'scripts'));
  writeFileSync(join(fixtureRoot, 'scripts', 'dogfood.sh'), readFileSync(script));
  writeFileSync(join(bin, 'cargo'), '#!/bin/sh\nexit 0\n', { mode: 0o755 });
  writeFileSync(join(bin, 'locus-mcp'), '#!/bin/sh\nexit 0\n', { mode: 0o755 });
  writeFileSync(join(bin, 'locus'), locusStub, { mode: 0o755 });
  for (const [name, content] of Object.entries(extraStubs)) {
    writeFileSync(join(bin, name), content, { mode: 0o755 });
  }
  return join(fixtureRoot, 'scripts', 'dogfood.sh');
}

test('a controlled TMPDIR contains the allocated dogfood home', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-dogfood-containment-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'controlled tmp root');
  const listing = join(root, 'root-listing');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');

  const probe = join(root, 'probe');
  const dogfood = fixture(root, `#!/usr/bin/env bash
case "$1" in
  init)
    # Containment must be physical, not just a matching prefix: the allocated
    # home is a real directory entry inside the controlled root, not a link.
    [[ -d "$LOCUS_HOME" && ! -L "$LOCUS_HOME" ]] || exit 71
    [[ -d "$TMPDIR/\${LOCUS_HOME##*/}" ]] || exit 72
    [[ "$PWD" == "$LOCUS_HOME/project" ]] || exit 73
    [[ "$(find "$TMPDIR" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')" == 1 ]] || exit 74
    ls -A "$TMPDIR" | sort > "$ROOT_LISTING"
    printf '%s\\n' "$LOCUS_HOME" > "$PROBE_FILE"
    ;;
  binding) ;;
  --version) ;;
  quickstart) exit 42 ;;
  *) exit 92 ;;
esac
`);
  const result = spawnSync('bash', [dogfood], {
    cwd: root,
    env: { ...process.env, TMPDIR: scratch, PROBE_FILE: probe, ROOT_LISTING: listing },
    encoding: 'utf8',
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 42, `the run must reach the controlled quickstart failure: ${result.stderr}`);
  const home = readFileSync(probe, 'utf8').trim();
  assert.equal(dirname(home), scratch);
  assert.match(basename(home), /^locus-dogfood\.[^/]+$/,
    'the leaf is one mktemp component beneath the controlled root');
  assert.ok(result.stdout.includes(`isolated LOCUS_HOME=${home}`),
    'the script must confirm the home it selected beneath the controlled root');
  assert.deepEqual(readFileSync(listing, 'utf8').trim().split('\n').sort(),
    [basename(home), 'keep'].sort(),
    'the controlled TMPDIR contains the allocated directory and nothing else');
  assert.throws(() => lstatSync(home), { code: 'ENOENT' },
    'the allocated home must be removed on exit');
  assert.deepEqual(readdirSync(scratch), ['keep'],
    'cleanup must preserve unrelated files in the controlled root');
});

// TMP, TEMP, the working directory and an inherited DOGFOOD_HOME are all
// plausible but wrong roots: the script must read TMPDIR alone when it chooses
// where the isolated home goes.
test('selection ignores TMP, TEMP, the working directory and an inherited DOGFOOD_HOME', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-dogfood-decoys-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'controlled root');
  const tmpDecoy = join(root, 'TMP decoy');
  const tempDecoy = join(root, 'TEMP decoy');
  const cwdDecoy = join(root, 'cwd decoy');
  const homeDecoy = join(root, 'DOGFOOD_HOME decoy');
  mkdirSync(scratch);
  mkdirSync(tmpDecoy);
  mkdirSync(tempDecoy);
  mkdirSync(cwdDecoy);
  mkdirSync(homeDecoy);
  writeFileSync(join(homeDecoy, 'keep'), 'caller-owned');

  const probe = join(root, 'probe');
  const dogfood = fixture(root, `#!/usr/bin/env bash
case "$1" in
  init)
    [[ -d "$LOCUS_HOME" && "\${LOCUS_HOME%/*}" == "$TMPDIR" ]] || exit 71
    printf '%s\\n' "$LOCUS_HOME" > "$PROBE_FILE"
    ;;
  binding) ;;
  --version) ;;
  quickstart) exit 42 ;;
  *) exit 92 ;;
esac
`);
  const result = spawnSync('bash', [dogfood], {
    cwd: cwdDecoy,
    env: { ...process.env, TMPDIR: scratch, TMP: tmpDecoy, TEMP: tempDecoy,
      DOGFOOD_HOME: homeDecoy, PROBE_FILE: probe },
    encoding: 'utf8',
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 42, `selection must succeed: ${result.stderr}`);
  const home = readFileSync(probe, 'utf8').trim();
  assert.equal(dirname(home), scratch,
    'TMPDIR — not TMP, TEMP, the working directory or DOGFOOD_HOME — is the selected root');
  assert.throws(() => lstatSync(home), { code: 'ENOENT' });
  for (const decoy of [tmpDecoy, tempDecoy, cwdDecoy]) {
    assert.deepEqual(readdirSync(decoy), [],
      `${basename(decoy)} must not receive the allocated directory`);
  }
  assert.deepEqual(readdirSync(homeDecoy), ['keep'],
    'the inherited DOGFOOD_HOME must survive untouched and unselected');
});

// A trailing slash does not change the boundary: the template becomes
// root//locus-dogfood.XXXXXX and the created entry still sits directly inside
// the controlled root.
test('a TMPDIR written with a trailing slash still contains the dogfood home', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-dogfood-trailing-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'controlled root');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'unrelated');

  const probe = join(root, 'probe');
  const dogfood = fixture(root, `#!/usr/bin/env bash
case "$1" in
  init)
    [[ -d "$LOCUS_HOME" && ! -L "$LOCUS_HOME" ]] || exit 71
    [[ -d "$TMPDIR/\${LOCUS_HOME##*/}" ]] || exit 72
    [[ "\${LOCUS_HOME%/*}" == "$TMPDIR" ]] || exit 73
    printf '%s\\n' "$LOCUS_HOME" > "$PROBE_FILE"
    ;;
  binding) ;;
  --version) ;;
  quickstart) exit 42 ;;
  *) exit 92 ;;
esac
`);
  const result = spawnSync('bash', [dogfood], {
    cwd: root,
    env: { ...process.env, TMPDIR: `${scratch}/`, PROBE_FILE: probe },
    encoding: 'utf8',
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 42, `selection must succeed: ${result.stderr}`);
  const home = readFileSync(probe, 'utf8').trim();
  assert.equal(resolve(dirname(home)), scratch,
    'the created home still resolves directly beneath the controlled root');
  assert.match(basename(home), /^locus-dogfood\.[^/]+$/);
  assert.throws(() => lstatSync(home), { code: 'ENOENT' });
  assert.deepEqual(readdirSync(scratch), ['keep']);
});

// A supplied TMPDIR is the boundary the caller chose: when it does not exist
// the run must stop, not silently substitute /tmp or any other root. The
// recording mktemp proves the supplied template is the only request the
// script ever makes.
test('a supplied TMPDIR that does not exist fails closed without a /tmp fallback', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-dogfood-missing-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const missing = join(root, 'missing root');
  const controlled = join(root, 'controlled');
  const templates = join(root, 'templates');
  mkdirSync(controlled);

  const probe = join(root, 'probe');
  const dogfood = fixture(root, `#!/usr/bin/env bash
case "$1" in
  init) touch "$PROBE_FILE" ;;
  *) exit 92 ;;
esac
`, {
    mktemp: `#!/usr/bin/env bash
last="\${!#}"
printf '%s\\n' "$last" >> "$MKTEMP_LOG"
PATH=/usr/bin:/bin mktemp "$@"
`,
  });
  const result = spawnSync('bash', [dogfood], {
    cwd: root,
    env: { ...process.env, TMPDIR: missing, PROBE_FILE: probe, MKTEMP_LOG: templates },
    encoding: 'utf8',
  });

  assert.equal(result.signal, null);
  assert.notEqual(result.status, 0, 'a missing controlled root must stop the run');
  assert.match(result.stderr, /mktemp/);
  assert.doesNotMatch(result.stdout, /isolated LOCUS_HOME/);
  assert.equal(readFileSync(templates, 'utf8').trim(), `${missing}/locus-dogfood.XXXXXX`,
    'the script must request the supplied root only — never retry beneath /tmp');
  assert.equal(existsSync(probe), false, 'locus must not run after allocation fails');
  assert.equal(existsSync(missing), false, 'the missing root must not be created for it');
  assert.deepEqual(readdirSync(controlled), [],
    'no fallback directory may be allocated beneath another root');
});

// The parent already holds other people's directories, including one named
// exactly like the script's own (locus-dogfood.XXXXXX) and one that is a
// symlink to a directory outside the parent. Cleanup may remove only the home
// this run created — never a lookalike sibling, the parent, or a link target.
test('cleanup removes only the home it created from a preexisting parent', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'locus-dogfood-preexisting-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const parent = join(root, 'preexisting parent');
  const outside = join(root, 'outside');
  mkdirSync(join(parent, 'sentinel', 'nested'), { recursive: true });
  mkdirSync(join(parent, 'locus-dogfood.PREEXIST'));
  mkdirSync(outside);
  writeFileSync(join(parent, 'sentinel', 'nested', 'data'), 'unrelated');
  writeFileSync(join(parent, 'locus-dogfood.PREEXIST', 'data'), 'lookalike');
  writeFileSync(join(outside, 'data'), 'linked');
  symlinkSync(outside, join(parent, 'locus-dogfood.LINKED'));
  const before = readdirSync(parent).sort();

  const probe = join(root, 'probe');
  const dogfood = fixture(root, `#!/usr/bin/env bash
case "$1" in
  init) printf '%s\\n' "$LOCUS_HOME" > "$PROBE_FILE" ;;
  binding) ;;
  --version) ;;
  quickstart) exit 42 ;;
  *) exit 92 ;;
esac
`);
  const result = spawnSync('bash', [dogfood], {
    cwd: root,
    env: { ...process.env, TMPDIR: parent, PROBE_FILE: probe },
    encoding: 'utf8',
  });

  assert.equal(result.signal, null);
  assert.equal(result.status, 42, `the run must reach the controlled quickstart failure: ${result.stderr}`);
  const home = readFileSync(probe, 'utf8').trim();
  assert.equal(dirname(home), parent);
  assert.ok(!before.includes(basename(home)), 'the run must create a new directory');
  assert.throws(() => lstatSync(home), { code: 'ENOENT' }, 'the created home must be removed');
  assert.deepEqual(readdirSync(parent).sort(), before,
    'every preexisting entry, lookalikes and links included, must remain');
  assert.equal(readFileSync(join(parent, 'sentinel', 'nested', 'data'), 'utf8'), 'unrelated');
  assert.equal(readFileSync(join(parent, 'locus-dogfood.PREEXIST', 'data'), 'utf8'), 'lookalike');
  assert.equal(readFileSync(join(outside, 'data'), 'utf8'), 'linked');
});
