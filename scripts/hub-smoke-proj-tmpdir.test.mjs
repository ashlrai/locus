// Run: node --test scripts/hub-smoke-proj-tmpdir.test.mjs
// SMOKE_PROJ must be created beneath the caller-selected TMPDIR and used
// verbatim as the smoke's working directory. These tests intercept mktemp
// as a real PATH executable — not a bash function like the sibling
// selection tests — so the project directory is observed the instant the
// real mktemp binary returns it, beneath exactly the root the script
// selected. A symlinked TMPDIR keeps its literal path (no
// canonicalization), nothing may land beneath ambient roots such as the
// caller's cwd or HOME (a bare `mktemp template` would create in cwd), and
// back-to-back runs must allocate distinct projects beneath the same root.
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
// for the SMOKE_PROJ allocation — records the created path after verifying
// it is a real, non-symlink directory directly beneath $SELECTED (the TMPDIR
// string as the script passed it). Stubs cover locus/jq/cargo; mktemp stays
// a real binary.
const shim = `#!/usr/bin/env bash
last="\${!#}"
printf '%s\\n' "$last" >> "$OBSERVED_TEMPLATES"
if [[ "$last" != */locus-hub-proj.XXXXXX ]]; then
  exec "$REAL_MKTEMP" "$@"
fi
path="$("$REAL_MKTEMP" "$@")" || exit 1
[[ -d "$path" && ! -L "$path" ]] || exit 90
[[ "\${path%/*}" == "$SELECTED" ]] || exit 91
printf '%s\\n' "$path" > "$OBSERVED_PATH"
printf '%s\\n' "$path"
`;

// At `locus init` — the script's first use of $SMOKE_PROJ — the directory
// must still be the recorded one: present, non-symlink, the only
// locus-hub-proj.* under TMPDIR, absent from the decoy roots, and resolved
// inside RESOLVED when TMPDIR is a symlink. When PRIOR_PROJ names a previous
// run's directory, it must already be gone. Once the script cd's into the
// project and writes .mcp.json, the first locus call from there records the
// working directory after confirming it is the recorded allocation.
const hook = `locus() {
  if [[ "\${1:-}" == init ]]; then
    [[ -d "$SMOKE_PROJ" && ! -L "$SMOKE_PROJ" ]] || return 92
    [[ "$SMOKE_PROJ" == "$(cat "$OBSERVED_PATH")" ]] || return 93
    local entries=("$TMPDIR"/locus-hub-proj.*)
    [[ \${#entries[@]} -eq 1 && "\${entries[0]}" == "$SMOKE_PROJ" ]] || return 94
    local decoy
    for decoy in "\${DECOY_HOME:-}" "\${DECOY_CWD:-}"; do
      [[ -n "$decoy" ]] || continue
      compgen -G "$decoy/locus-hub-proj.*" >/dev/null && return 95
    done
    [[ -z "\${RESOLVED:-}" || -d "$RESOLVED/\${SMOKE_PROJ##*/}" ]] || return 96
    [[ -z "\${PRIOR_PROJ:-}" || ! -e "$PRIOR_PROJ" ]] || return 97
    printf '%s\\n' "$SMOKE_PROJ" > "$OBSERVED_USED"
  fi
  if [[ ! -e "$OBSERVED_CWD" && "\${PWD##*/}" == locus-hub-proj.* && -f .mcp.json ]]; then
    [[ . -ef "$(cat "$OBSERVED_PATH")" ]] || return 98
    printf '%s\\n' "$PWD" > "$OBSERVED_CWD"
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

function fixture(t, prefix = '/tmp/lhs-proj-tmpdir-') {
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
    cwd: join(root, 'cwd'),
  };
}

function run(f, env) {
  const result = spawnSync('bash', [script], {
    ...env,
    env: {
      ...process.env, BASH_ENV: f.hook, REAL_MKTEMP: realMktemp,
      OBSERVED_TEMPLATES: f.templates, OBSERVED_PATH: f.created,
      OBSERVED_USED: f.used, OBSERVED_CWD: f.cwd,
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

function assertProject(f, selected) {
  const created = readFileSync(f.created, 'utf8').trim();
  assert.equal(dirname(created), selected, 'created project must sit directly beneath the selected TMPDIR');
  assert.match(basename(created), /^locus-hub-proj\.[A-Za-z0-9]{6}$/);
  assert.equal(readFileSync(f.used, 'utf8').trim(), created, 'init must see the directory mktemp created');
  assert.equal(readFileSync(f.cwd, 'utf8').trim(), created, 'checks must run inside the created project');
  assert.equal(existsSync(created), false, 'temporary project must be removed on exit');
  return created;
}

test('SMOKE_PROJ is created beneath the selected TMPDIR', (t) => {
  const f = fixture(t);
  const selected = join(f.root, 'selected');
  mkdirSync(selected);
  writeFileSync(join(selected, 'keep'), 'sentinel');
  run(f, { env: { TMPDIR: selected, SELECTED: selected } });
  assertProject(f, selected);
  // Every mktemp allocation — home, project and error file — selects the
  // same caller-provided root.
  const templates = readFileSync(f.templates, 'utf8').trim().split('\n');
  assert.equal(templates.length, 3);
  for (const template of templates) {
    assert.equal(dirname(template), selected);
  }
  assert.deepEqual(readdirSync(selected), ['keep'], 'selected root keeps only its own entries');
  assert.equal(readFileSync(join(selected, 'keep'), 'utf8'), 'sentinel');
});

test('SMOKE_PROJ uses a symlinked TMPDIR verbatim', (t) => {
  const f = fixture(t);
  const target = join(f.root, 'target');
  const link = join(f.root, 'link');
  mkdirSync(target);
  symlinkSync(target, link);
  // The init stub sees the directory through the symlink path and inside the
  // target directory it resolves to — same allocation, beneath the selected root.
  run(f, { env: { TMPDIR: link, SELECTED: link, RESOLVED: target } });
  const created = assertProject(f, link);
  assert.ok(lstatSync(link).isSymbolicLink(), 'the TMPDIR symlink itself must survive');
  assert.equal(dirname(created), link, 'the selected root is used verbatim, not canonicalized');
  assert.deepEqual(readdirSync(target), [], 'the project materialized in — and was removed from — the target');
});

test('SMOKE_PROJ lands beneath the selected TMPDIR, not ambient roots', (t) => {
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
  assertProject(f, selected);
  assert.deepEqual(readdirSync(selected), ['keep'], 'selected root keeps only its own entries');
  for (const decoy of [decoyHome, decoyCwd]) {
    assert.deepEqual(readdirSync(decoy), ['keep'], `decoy root ${decoy} must gain no project directory`);
    assert.equal(readFileSync(join(decoy, 'keep'), 'utf8'), 'sentinel');
  }
});

test('SMOKE_PROJ is created as one directory beneath a space-containing, glob-like TMPDIR', (t) => {
  const f = fixture(t);
  const selected = join(f.root, 'tmp dir [a] *');
  const split = join(f.root, 'tmp');
  mkdirSync(selected);
  mkdirSync(split);
  writeFileSync(join(split, 'keep'), 'sentinel');
  run(f, { env: { TMPDIR: selected, SELECTED: selected } });
  const created = assertProject(f, selected);
  assert.equal(dirname(created), selected, 'the spaced root is used verbatim, not word-split');
  assert.deepEqual(readdirSync(selected), [], 'selected root is clean after the EXIT trap');
  assert.deepEqual(readdirSync(split), ['keep'], 'no fragment lands beneath the split-off parent');
});

// Each run must allocate its own project beneath the same temporary root:
// a fixed or recycled path would let one smoke's .mcp.json and working tree
// observe — or delete — another run's project.
test('back-to-back runs allocate distinct projects beneath the same TMPDIR', (t) => {
  const f = fixture(t);
  const selected = join(f.root, 'selected');
  mkdirSync(selected);

  const runs = [0, 1].map((index) => {
    const files = {
      created: join(f.root, `created-${index}`), used: join(f.root, `used-${index}`),
      cwd: join(f.root, `cwd-${index}`),
    };
    const result = spawnSync('bash', [script], {
      env: {
        ...process.env, BASH_ENV: f.hook, REAL_MKTEMP: realMktemp,
        OBSERVED_TEMPLATES: join(f.root, `templates-${index}`),
        OBSERVED_PATH: files.created, OBSERVED_USED: files.used, OBSERVED_CWD: files.cwd,
        TMPDIR: selected, SELECTED: selected,
        PRIOR_PROJ: index === 1 ? readFileSync(join(f.root, 'created-0'), 'utf8').trim() : '',
        PATH: `${join(f.root, 'bin')}:${process.env.PATH}`,
      },
      encoding: 'utf8',
      timeout: 15_000,
    });
    assert.equal(result.error, undefined);
    assert.equal(result.signal, null);
    assert.equal(result.status, 0, `run ${index} stderr:\n${result.stderr}`);
    assert.match(result.stdout, /^== hub-smoke OK ==$/m);
    assert.doesNotMatch(result.stdout, /^skip\s/m, 'all smoke checks must run');
    return {
      created: readFileSync(files.created, 'utf8').trim(),
      used: readFileSync(files.used, 'utf8').trim(),
      cwd: readFileSync(files.cwd, 'utf8').trim(),
    };
  });

  assert.notEqual(runs[0].created, runs[1].created, 'separate runs must create distinct directories');
  for (const observed of runs) {
    assert.equal(observed.used, observed.created, 'each run must use the project it created');
    assert.equal(observed.cwd, observed.created, 'each run must work inside its own project');
    assert.equal(dirname(observed.created), selected);
    assert.equal(existsSync(observed.created), false, 'each project must be removed on exit');
  }
  assert.deepEqual(readdirSync(selected), [], 'both runs must leave TMPDIR empty');
});
