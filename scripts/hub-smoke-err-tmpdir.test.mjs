// Run: node --test scripts/hub-smoke-err-tmpdir.test.mjs
// SMOKE_ERR must be created beneath the caller-selected TMPDIR, used
// verbatim. These tests intercept mktemp as a real PATH executable — not a
// bash function like the sibling selection tests — so the file is observed
// the instant the real mktemp binary returns it, beneath exactly the root
// the script selected. A symlinked TMPDIR keeps its literal path (no
// canonicalization), and nothing may land beneath ambient roots such as the
// caller's cwd or HOME (a bare `mktemp template` would create in cwd).
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import {
  existsSync, lstatSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, symlinkSync, writeFileSync,
} from 'node:fs';
import { basename, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-smoke.sh');
const realMktemp = execFileSync('which', ['mktemp'], { encoding: 'utf8' }).trim();

// The mktemp shim logs every template, delegates to the real binary, then —
// for the SMOKE_ERR allocation — records the created path and verifies it is
// a regular empty file directly beneath $SELECTED (the TMPDIR string as the
// script passed it). Stubs cover locus/jq/cargo; mktemp stays a real binary.
const shim = `#!/usr/bin/env bash
last="\${!#}"
printf '%s\\n' "$last" >> "$OBSERVED_TEMPLATES"
if [[ "$last" != */locus-hub-smoke-err.XXXXXX ]]; then
  exec "$REAL_MKTEMP" "$@"
fi
path="$("$REAL_MKTEMP" "$@")" || exit 1
[[ -f "$path" && ! -L "$path" && ! -s "$path" ]] || exit 90
[[ "\${path%/*}" == "$SELECTED" ]] || exit 91
printf '%s\\n' "$path" > "$OBSERVED_PATH"
printf '%s\\n' "$path"
`;

// At the first init call — the script's first use of $SMOKE_ERR — the file
// must still be the recorded one: present, regular, empty, the only
// locus-hub-smoke-err.* under TMPDIR, and absent from the decoy roots.
const hook = `locus() {
  if [[ "\${1:-}" == init ]]; then
    [[ -f "$SMOKE_ERR" && ! -L "$SMOKE_ERR" && ! -s "$SMOKE_ERR" ]] || return 92
    [[ "$SMOKE_ERR" == "$(cat "$OBSERVED_PATH")" ]] || return 93
    local entries=("$TMPDIR"/locus-hub-smoke-err.*)
    [[ \${#entries[@]} -eq 1 && "\${entries[0]}" == "$SMOKE_ERR" ]] || return 94
    local decoy
    for decoy in "\${DECOY_HOME:-}" "\${DECOY_CWD:-}"; do
      [[ -n "$decoy" ]] || continue
      compgen -G "$decoy/locus-hub-smoke-err.*" >/dev/null && return 95
    done
    printf '%s\\n' "$SMOKE_ERR" > "$OBSERVED_USED"
  fi
  if [[ "\${1:-} \${2:-}" == 'status --oneline' ]]; then
    printf '%s\\n' personal:tenant
  else
    printf '%s\\n' '{}'
  fi
}
jq() { cat >/dev/null; printf '%s\\n' grant; }
cargo() { return 0; }
`;

function fixture(t, prefix = '/tmp/lhs-err-tmpdir-') {
  const root = mkdtempSync(prefix);
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const bin = join(root, 'bin');
  mkdirSync(bin);
  writeFileSync(join(bin, 'mktemp'), shim, { mode: 0o755 });
  writeFileSync(join(root, 'hook.sh'), hook);
  return {
    root,
    hook: join(root, 'hook.sh'),
    templates: join(root, 'templates'),
    created: join(root, 'created'),
    used: join(root, 'used'),
  };
}

function run(f, env) {
  const result = spawnSync('bash', [script], {
    ...env,
    env: {
      ...process.env, BASH_ENV: f.hook, REAL_MKTEMP: realMktemp,
      OBSERVED_TEMPLATES: f.templates, OBSERVED_PATH: f.created, OBSERVED_USED: f.used,
      PATH: `${join(f.root, 'bin')}:${process.env.PATH}`, ...(env.env ?? {}),
    },
    encoding: 'utf8',
    timeout: 15_000,
  });
  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 0, `stderr:\n${result.stderr}`);
  assert.match(result.stdout, /^== hub-smoke OK ==$/m);
  assert.doesNotMatch(result.stdout, /^skip\s/m, 'all smoke checks must run');
  return result;
}

function assertErrorFile(f, selected) {
  const created = readFileSync(f.created, 'utf8').trim();
  assert.equal(dirname(created), selected, 'created file must sit directly beneath the selected TMPDIR');
  assert.match(basename(created), /^locus-hub-smoke-err\.[A-Za-z0-9]{6}$/);
  assert.equal(readFileSync(f.used, 'utf8').trim(), created, 'init must use the file mktemp created');
  assert.equal(existsSync(created), false, 'temporary error file must be removed on exit');
  return created;
}

test('SMOKE_ERR is created beneath the selected TMPDIR', (t) => {
  const f = fixture(t);
  const selected = join(f.root, 'selected');
  mkdirSync(selected);
  writeFileSync(join(selected, 'keep'), 'sentinel');
  run(f, { env: { TMPDIR: selected, SELECTED: selected } });
  assertErrorFile(f, selected);
  // Every mktemp allocation — home, project and error file — selects the
  // same caller-provided root.
  const templates = readFileSync(f.templates, 'utf8').trim().split('\n');
  for (const template of templates) {
    assert.equal(dirname(template), selected);
  }
  assert.deepEqual(readdirSync(selected), ['keep'], 'selected root keeps only its own entries');
  assert.equal(readFileSync(join(selected, 'keep'), 'utf8'), 'sentinel');
});

test('SMOKE_ERR uses a symlinked TMPDIR verbatim', (t) => {
  const f = fixture(t);
  const target = join(f.root, 'target');
  const link = join(f.root, 'link');
  mkdirSync(target);
  symlinkSync(target, link);
  // The init stub sees the file through the symlink path and inside the
  // target directory it resolves to — same file, beneath the selected root.
  writeFileSync(join(f.root, 'hook.sh'), `locus() {
  if [[ "\${1:-}" == init ]]; then
    [[ -f "$SMOKE_ERR" && ! -L "$SMOKE_ERR" && ! -s "$SMOKE_ERR" ]] || return 92
    [[ "$SMOKE_ERR" == "$(cat "$OBSERVED_PATH")" ]] || return 93
    [[ "\${SMOKE_ERR%/*}" == "$TMPDIR" ]] || return 94
    [[ -f "$RESOLVED/\${SMOKE_ERR##*/}" ]] || return 95
    printf '%s\\n' "$SMOKE_ERR" > "$OBSERVED_USED"
  fi
  if [[ "\${1:-} \${2:-}" == 'status --oneline' ]]; then
    printf '%s\\n' personal:tenant
  else
    printf '%s\\n' '{}'
  fi
}
jq() { cat >/dev/null; printf '%s\\n' grant; }
cargo() { return 0; }
`);
  run(f, { env: { TMPDIR: link, SELECTED: link, RESOLVED: target } });
  const created = assertErrorFile(f, link);
  assert.ok(lstatSync(link).isSymbolicLink(), 'the TMPDIR symlink itself must survive');
  assert.equal(dirname(created), link, 'the selected root is used verbatim, not canonicalized');
  assert.deepEqual(readdirSync(target), [], 'the file materialized in — and was removed from — the target');
});

test('SMOKE_ERR lands beneath the selected TMPDIR, not ambient roots', (t) => {
  const f = fixture(t);
  const selected = join(f.root, 'selected');
  const decoyHome = join(f.root, 'decoy home');
  const decoyCwd = join(f.root, 'decoy cwd');
  for (const dir of [selected, decoyHome, decoyCwd]) {
    mkdirSync(dir);
    writeFileSync(join(dir, 'keep'), 'sentinel');
  }
  // A bare `mktemp template` creates in the caller's cwd; a root derived
  // from HOME lands there instead. Both decoys must stay untouched mid-run.
  run(f, {
    cwd: decoyCwd,
    env: { TMPDIR: selected, SELECTED: selected, HOME: decoyHome, DECOY_HOME: decoyHome, DECOY_CWD: decoyCwd },
  });
  assertErrorFile(f, selected);
  assert.deepEqual(readdirSync(selected), ['keep'], 'selected root keeps only its own entries');
  for (const decoy of [decoyHome, decoyCwd]) {
    assert.deepEqual(readdirSync(decoy), ['keep'], `decoy root ${decoy} must gain no error file`);
    assert.equal(readFileSync(join(decoy, 'keep'), 'utf8'), 'sentinel');
  }
});

test('SMOKE_ERR is created as one file beneath a space-containing, glob-like TMPDIR', (t) => {
  const f = fixture(t);
  const selected = join(f.root, 'tmp dir [a] *');
  const split = join(f.root, 'tmp');
  mkdirSync(selected);
  mkdirSync(split);
  writeFileSync(join(split, 'keep'), 'sentinel');
  run(f, { env: { TMPDIR: selected, SELECTED: selected } });
  const created = assertErrorFile(f, selected);
  assert.equal(dirname(created), selected, 'the spaced root is used verbatim, not word-split');
  assert.deepEqual(readdirSync(selected), [], 'selected root is clean after the EXIT trap');
  assert.deepEqual(readdirSync(split), ['keep'], 'no fragment lands beneath the split-off parent');
});
