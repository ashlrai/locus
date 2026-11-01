// Run: node --test scripts/hub-smoke.test.mjs
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-smoke.sh');
const realMktemp = execFileSync('which', ['mktemp'], { encoding: 'utf8' }).trim();

test('header documents the local smoke command and prerequisites', () => {
  const header = readFileSync(script, 'utf8').split('set -euo pipefail', 1)[0];
  assert.match(header, /repository root[^\n]*\.\/scripts\/hub-smoke\.sh/);
  for (const prerequisite of ['Bash', 'jq', 'Rust', 'Cargo', 'mktemp', '/dev/urandom']) {
    assert.ok(header.includes(prerequisite), `missing prerequisite: ${prerequisite}`);
  }
  assert.match(header, /writable temporary storage and target\//);
  assert.match(header, /throwaway LOCUS_HOME/);
});

test('prerequisites document tracks the script environment and checked files', () => {
  const source = readFileSync(script, 'utf8');
  const doc = readFileSync(join(dirname(script), '../docs/hub-smoke-prerequisites.md'), 'utf8');
  for (const variable of ['HOME', 'PATH', 'TMPDIR', 'LOCUS_CONTROL_CAPABILITY',
    'LOCUS_AUTHORITY_BROKER_START_TIMEOUT_MS', 'LOCUS_HOME', 'LOCUS_SESSION_ID']) {
    assert.ok(source.includes(variable) && doc.includes(`\`${variable}\``), `missing environment variable: ${variable}`);
  }
  for (const match of source.matchAll(/for f in ([^;]+); do/g)) {
    for (const file of match[1].trim().split(/\s+/)) {
      const path = file.endsWith('.schema.json') ? `schema/${file}` : file;
      assert.ok(doc.includes(path), `missing checked file: ${path}`);
    }
  }
  assert.match(doc, /overall smoke may still succeed/);
  assert.match(doc, /valid JSON response can pass even if/);
  assert.match(doc, /== hub-smoke OK ==/);
});

test('output guide covers the script messages and check names', () => {
  const source = readFileSync(script, 'utf8');
  const doc = readFileSync(join(dirname(script), '../docs/hub-smoke-output.md'), 'utf8');
  assert.match(doc, /\.\/scripts\/hub-smoke\.sh/);
  const messages = [
    'error: hub-smoke could not create temporary home',
    'error: hub-smoke could not create temporary project directory',
    'error: hub-smoke could not create temporary error file',
    'error: jq is required',
    'building locus with agent report…',
    'FAIL  status --oneline expected alias:tenant, got:',
    'ok    mcp mint --json grant contract (token validated, not echoed)',
    'FAIL  mcp mint --json grant contract',
    'ok    mcp list --json values-free roster',
    'FAIL  mcp list --json roster contract',
    'ok    mcp revoke removes grant from roster',
    'FAIL  mcp revoke contract',
    'skip  mcp mint/list/revoke (locus mcp subcommand unavailable)',
    '== hub-smoke FAILED ==',
    '== hub-smoke OK ==',
  ];
  for (const message of messages) {
    assert.ok(source.includes(message) && doc.includes(message), `missing message: ${message}`);
  }
  for (const match of source.matchAll(/check_json "([^"]+)"/g)) {
    assert.ok(doc.includes(`\`${match[1]}\``), `missing check: ${match[1]}`);
  }
  for (const match of source.matchAll(/for f in ([^;]+); do/g)) {
    for (const file of match[1].trim().split(/\s+/)) {
      const path = file.endsWith('.schema.json') ? `schema/${file}` : file;
      assert.ok(doc.includes(path), `missing checked file: ${path}`);
    }
  }
  for (const phrase of ['not JSON (exit=', 'jq predicate failed:',
    'possible secret material in output', 'attempt <1 or 2> failed; retrying…',
    'failed after 3 attempts', 'final banner', 'not itself a pass criterion']) {
    assert.ok(doc.includes(phrase), `missing output behavior: ${phrase}`);
  }
});

test('resource map covers temporary allocations and their lifecycle', () => {
  const source = readFileSync(script, 'utf8');
  const doc = readFileSync(join(dirname(script), '../docs/hub-smoke-resource-map.md'), 'utf8');
  const allocations = [...source.matchAll(/if ! (SMOKE_\w+)="\$\(mktemp(?: -d)? "\$\{TMPDIR:-\/tmp\}\/([^"]+)"\)"/g)];
  assert.equal(allocations.length, 3, 'review the map when temporary allocations change');
  for (const [, variable, template] of allocations) {
    const row = doc.split('\n').find((line) => line.startsWith(`| \`${variable}\``));
    assert.ok(row, `missing resource row: ${variable}`);
    assert.ok(row.includes(template), `missing template: ${template}`);
    assert.equal(row.split('|').length, 6, `${variable} needs creation, use and cleanup cells`);
    assert.ok(source.includes(`"$${variable}"`), `${variable} must be used by the script`);
  }
  for (const marker of ['cleanup() { rm -rf -- "$SMOKE_HOME" "$SMOKE_PROJ" "$SMOKE_ERR"; }',
    'trap cleanup EXIT', 'locus init --with-samples', 'locus pin personal',
    'locus leave', '> .mcp.json', 'locus mcp mint --binding personal', 'locus mcp revoke']) {
    assert.ok(source.includes(marker), `script lifecycle changed: ${marker}`);
  }
  for (const item of ['Sample bindings', 'Authority broker and Unix socket', 'Active pin',
    '`.mcp.json`', 'MCP grant, delegated session and bearer token', 'Control capability',
    'uncatchable kill', 'asynchronous', '`target/`']) {
    assert.ok(doc.includes(item), `missing lifecycle coverage: ${item}`);
  }
});

test('temporary resource guide matches allocation types and prefixes', () => {
  const source = readFileSync(script, 'utf8');
  const doc = readFileSync(join(dirname(script), '../docs/hub-smoke-temp-resources.md'), 'utf8');
  const allocations = [...source.matchAll(/if ! (SMOKE_\w+)="\$\(mktemp( -d)? "\$\{TMPDIR:-\/tmp\}\/([^"]+)"\)"/g)];
  const expected = [
    ['SMOKE_HOME', ' -d', 'locus-hub-smoke.XXXXXX', 'Directory'],
    ['SMOKE_PROJ', ' -d', 'locus-hub-proj.XXXXXX', 'Directory'],
    ['SMOKE_ERR', undefined, 'locus-hub-smoke-err.XXXXXX', 'Regular file'],
  ];
  assert.equal(allocations.length, expected.length, 'review the guide when allocations change');
  for (const [index, [variable, flag, template, kind]] of expected.entries()) {
    assert.deepEqual(allocations[index].slice(1), [variable, flag, template]);
    const row = doc.split('\n').find((line) => line.startsWith(`| \`${variable}\``));
    assert.ok(row, `missing resource row: ${variable}`);
    assert.ok(row.includes(`| ${kind} |`), `wrong kind for ${variable}`);
    assert.ok(row.includes(`\`${template}\``), `missing template: ${template}`);
    assert.ok(row.includes(`\`${template.slice(0, -6)}\``), `missing prefix: ${variable}`);
  }
  assert.match(doc, /`TMPDIR`.*`\/tmp`/);
  assert.match(doc, /`EXIT` trap/);
  assert.match(doc, /uncatchable kill/);
});

test('temporary resources reference identifies each allocation and its purpose', () => {
  const source = readFileSync(script, 'utf8');
  const doc = readFileSync(join(dirname(script), '../docs/hub-smoke-resources.md'), 'utf8');
  const allocations = [...source.matchAll(/if ! (SMOKE_\w+)="\$\(mktemp( -d)? "\$\{TMPDIR:-\/tmp\}\/([^"]+)"\)"/g)];
  const expected = [
    ['SMOKE_HOME', ' -d', 'locus-hub-smoke.XXXXXX', 'Directory', 'export LOCUS_HOME="$SMOKE_HOME"', /LOCUS_HOME.*sample bindings.*pin/],
    ['SMOKE_PROJ', ' -d', 'locus-hub-proj.XXXXXX', 'Directory', 'cd "$SMOKE_PROJ"', /working project.*\.mcp\.json.*agent report/i],
    ['SMOKE_ERR', undefined, 'locus-hub-smoke-err.XXXXXX', 'File', ': >"$SMOKE_ERR"', /init and pin stderr.*retry.*truncated/],
  ];
  assert.equal(allocations.length, expected.length, 'review the reference when allocations change');
  for (const [index, [variable, flag, template, kind, use, purpose]] of expected.entries()) {
    assert.deepEqual(allocations[index].slice(1), [variable, flag, template]);
    assert.ok(source.includes(use), `script use changed: ${variable}`);
    const row = doc.split('\n').find((line) => line.startsWith(`| \`${variable}\``));
    assert.ok(row, `missing resource: ${variable}`);
    assert.ok(row.includes(`| ${kind} |`) && row.includes(`\`${template}\``));
    assert.match(row, purpose);
  }
});

test('resource roles cite each allocation and explain its use', () => {
  const lines = readFileSync(script, 'utf8').split('\n');
  const doc = readFileSync(join(dirname(script), '../docs/hub-smoke-resource-roles.md'), 'utf8');
  const allocations = lines.flatMap((line, index) => {
    const match = line.match(/^if ! (SMOKE_\w+)="\$\(mktemp(?: -d)? /);
    return match ? [[match[1], index + 1]] : [];
  });
  assert.equal(allocations.length, 3, 'review resource roles when allocations change');
  for (const [variable, line] of allocations) {
    const row = doc.split('\n').find((entry) => entry.startsWith(`| \`${variable}\``));
    assert.ok(row, `missing resource role: ${variable}`);
    assert.ok(row.includes(`../scripts/hub-smoke.sh#L${line}`), `wrong creation site: ${variable}`);
  }
  for (const [variable, marker, role] of [
    ['SMOKE_HOME', 'export LOCUS_HOME="$SMOKE_HOME"', /LOCUS_HOME.*isolating/],
    ['SMOKE_PROJ', 'cd "$SMOKE_PROJ"', /working project.*\.mcp\.json/],
    ['SMOKE_ERR', ': >"$SMOKE_ERR"', /stderr.*truncated/],
  ]) {
    assert.ok(lines.some((line) => line.includes(marker)), `script use changed: ${variable}`);
    assert.match(doc.split('\n').find((entry) => entry.startsWith(`| \`${variable}\``)), role);
  }
});

function smokeCommandGuard(root) {
  const guard = join(root, 'guard.sh');
  const marker = join(root, 'smoke-command-ran');
  // Functions also intercept binaries in the script's prepended target directories.
  writeFileSync(guard, `locus() { : > "$SMOKE_COMMAND_MARKER"; return 1; }
cargo() { : > "$SMOKE_COMMAND_MARKER"; return 1; }
jq() { : > "$SMOKE_COMMAND_MARKER"; return 1; }
`);
  return { BASH_ENV: guard, SMOKE_COMMAND_MARKER: marker };
}

for (const [description, directory] of [
  ['custom TMPDIR', 'custom tmp'],
  ['TMPDIR with spaces and shell pattern characters', 'a [*] b'],
]) test(`successful smoke uses and cleans a ${description}`, (t) => {
  // The authority broker uses a Unix socket, so keep the temporary path short.
  const root = mkdtempSync('/tmp/lhs-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const bin = join(root, 'bin');
  const scratch = join(root, directory);
  const record = join(root, 'created');
  mkdirSync(bin);
  mkdirSync(scratch);
  const sentinels = ['keep', '.hidden', 'locus-hub-smoke.unrelated',
    'locus-hub-proj.unrelated', 'locus-hub-smoke-err.unrelated'];
  for (const name of sentinels) writeFileSync(join(scratch, name), 'sentinel');
  writeFileSync(join(bin, 'mktemp'), `#!/usr/bin/env bash
case "\${1:-}" in
  -d) [[ $# -eq 2 && "$2" == "$TMPDIR"/locus-hub-*.XXXXXX ]] || exit 72 ;;
  *)  [[ $# -eq 1 && "$1" == "$TMPDIR"/locus-hub-*.XXXXXX ]] || exit 72 ;;
esac
path="$("$REAL_MKTEMP" "$@")" || exit
case "$path" in
  "$TMPDIR"/locus-hub-smoke.*|"$TMPDIR"/locus-hub-proj.*|"$TMPDIR"/locus-hub-smoke-err.*)
    if [[ "\${1:-}" == -d && -d "$path" ]]; then
      kind=directory
    elif [[ "\${1:-}" != -d && -f "$path" ]]; then
      kind=file
    else
      exit 1
    fi
    printf '%s\\t%s\\n' "$kind" "$path" >> "$MKTEMP_RECORD" ;;
esac
printf '%s\\n' "$path"
`, { mode: 0o755 });
  const env = { ...process.env, TMPDIR: scratch, PATH: `${bin}:${process.env.PATH}`,
    REAL_MKTEMP: realMktemp, MKTEMP_RECORD: record };
  delete env.BASH_ENV;
  const result = spawnSync('bash', [script], {
    env,
    encoding: 'utf8',
    maxBuffer: 4 * 1024 * 1024,
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 0, 'smoke must succeed');
  assert.match(result.stdout, /^== hub-smoke OK ==$/m);
  assert.doesNotMatch(result.stdout, /^skip\s/m, 'all smoke checks must run');
  for (const name of ['agent report --json', 'doctor --json', 'whoami --json',
    'status --json', 'agent report unpinned']) {
    assert.ok(result.stdout.includes(`ok    ${name} (exit=`), `missing success: ${name}`);
  }
  for (const message of ['ok    status --oneline (',
    'ok    mcp mint --json grant contract (token validated, not echoed)',
    'ok    mcp list --json values-free roster', 'ok    mcp revoke removes grant from roster']) {
    assert.ok(result.stdout.includes(message), `missing success: ${message}`);
  }
  const resources = readFileSync(record, 'utf8').trim().split('\n').map((line) => line.split('\t'));
  assert.equal(resources.length, 3);
  for (const [i, [kind, path]] of resources.entries()) {
    const [expectedKind, prefix] = [
      ['directory', 'locus-hub-smoke.'], ['directory', 'locus-hub-proj.'],
      ['file', 'locus-hub-smoke-err.'],
    ][i];
    assert.equal(kind, expectedKind);
    assert.equal(dirname(path), scratch);
    assert.ok(path.slice(scratch.length + 1).startsWith(prefix));
    assert.equal(existsSync(path), false, `${prefix} must be removed`);
  }
  assert.deepEqual(readdirSync(scratch).sort(), sentinels.sort());
  for (const name of sentinels) assert.equal(readFileSync(join(scratch, name), 'utf8'), 'sentinel');
});

test('successful smoke removes its temporary error file', (t) => {
  // Keep the broker socket path short and isolate the error file from other runs.
  const root = mkdtempSync('/tmp/lhs-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'temporary files');
  const record = join(root, 'error-file');
  const hook = join(root, 'observe-pin.sh');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'locus-hub-smoke-err.unrelated'), 'keep');
  writeFileSync(hook, `locus() {
  if [[ "$*" == 'pin personal' ]]; then
    [[ -f "$SMOKE_ERR" ]] || return 90
    printf 'synthetic pin diagnostic\\n' >&2
    command locus "$@" || return
    grep -q '^synthetic pin diagnostic$' "$SMOKE_ERR" || return 91
    printf '%s\\n' "$SMOKE_ERR" > "$SMOKE_ERR_RECORD"
  else
    command locus "$@"
  fi
}
`);
  const result = spawnSync('bash', [script], {
    env: { ...process.env, TMPDIR: scratch, BASH_ENV: hook, SMOKE_ERR_RECORD: record },
    encoding: 'utf8',
    maxBuffer: 4 * 1024 * 1024,
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 0, 'smoke must complete successfully');
  assert.match(result.stdout, /^== hub-smoke OK ==$/m);
  assert.doesNotMatch(result.stdout, /^skip\s/m, 'all smoke checks must run');
  const errorFile = readFileSync(record, 'utf8').trim();
  assert.equal(dirname(errorFile), scratch);
  assert.match(errorFile.slice(scratch.length + 1), /^locus-hub-smoke-err\./);
  assert.equal(existsSync(errorFile), false, 'the observed error file must be removed');
  assert.equal(readFileSync(join(scratch, 'locus-hub-smoke-err.unrelated'), 'utf8'), 'keep');
});

test('a spaced TMPDIR holds all three resources until a build failure cleans them', (t) => {
  const root = mkdtempSync('/tmp/lhs-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'custom temporary directory');
  const record = join(root, 'created');
  const marker = join(root, 'build-saw-resources');
  const hook = join(root, 'hook.sh');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'sentinel');
  writeFileSync(hook, `mktemp() {
  local path kind
  path="$(command mktemp "$@")" || return
  if [[ "\${1:-}" == -d && -d "$path" ]]; then
    kind=directory
  elif [[ "\${1:-}" != -d && -f "$path" ]]; then
    kind=file
  else
    return 1
  fi
  printf '%s\\t%s\\n' "$kind" "$path" >> "$SMOKE_RECORD"
  printf '%s\\n' "$path"
}
jq() { return 0; }
locus() { return 1; }
cargo() {
  local kind path count=0
  while IFS=$'\\t' read -r kind path; do
    case "$kind" in
      directory) [[ -d "$path" ]] || return 74 ;;
      file) [[ -f "$path" ]] || return 74 ;;
      *) return 74 ;;
    esac
    count=$((count + 1))
  done < "$SMOKE_RECORD"
  [[ $count -eq 3 ]] || return 74
  : > "$SMOKE_MARKER"
  return 73
}
`);
  const result = spawnSync('bash', [script], {
    env: { ...process.env, BASH_ENV: hook, TMPDIR: scratch, SMOKE_RECORD: record, SMOKE_MARKER: marker },
    encoding: 'utf8',
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 73, 'the simulated build failure must propagate');
  assert.ok(existsSync(marker), 'all resources must exist when the build starts');
  const resources = readFileSync(record, 'utf8').trim().split('\n').map((line) => line.split('\t'));
  assert.equal(resources.length, 3);
  for (const [index, [kind, path]] of resources.entries()) {
    const [expectedKind, prefix] = [
      ['directory', 'locus-hub-smoke.'], ['directory', 'locus-hub-proj.'],
      ['file', 'locus-hub-smoke-err.'],
    ][index];
    assert.equal(kind, expectedKind);
    assert.equal(dirname(path), scratch);
    assert.ok(path.slice(scratch.length + 1).startsWith(prefix));
    assert.equal(existsSync(path), false, `${prefix} must be removed`);
  }
  assert.deepEqual(readdirSync(scratch), ['keep']);
  assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'sentinel');
});

test('a TMPDIR with spaces is cleaned after partial setup failure', (t) => {
  const root = mkdtempSync('/tmp/lhs-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const bin = join(root, 'bin');
  const scratch = join(root, 'temporary resources with spaces');
  const record = join(root, 'created');
  mkdirSync(bin);
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'sentinel');
  const guard = smokeCommandGuard(root);
  writeFileSync(join(bin, 'mktemp'), `#!/usr/bin/env bash
case "$*" in
  *locus-hub-smoke-err.XXXXXX) exit 1 ;;
esac
path="$("$REAL_MKTEMP" "$@")" || exit
[[ -d "$path" ]] || exit 1
printf '%s\\n' "$path" >> "$MKTEMP_RECORD"
printf '%s\\n' "$path"
`, { mode: 0o755 });
  const result = spawnSync('bash', [script], {
    env: { ...process.env, ...guard, TMPDIR: scratch, PATH: `${bin}:${process.env.PATH}`,
      REAL_MKTEMP: realMktemp, MKTEMP_RECORD: record },
    encoding: 'utf8',
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /error: hub-smoke could not create temporary error file/);
  assert.equal(result.stdout, '');
  assert.equal(existsSync(guard.SMOKE_COMMAND_MARKER), false);
  const paths = readFileSync(record, 'utf8').trim().split('\n');
  assert.equal(paths.length, 2);
  for (const [index, prefix] of ['locus-hub-smoke.', 'locus-hub-proj.'].entries()) {
    assert.equal(dirname(paths[index]), scratch);
    assert.ok(paths[index].slice(scratch.length + 1).startsWith(prefix));
    assert.equal(existsSync(paths[index]), false, `${prefix} must be removed`);
  }
  assert.deepEqual(readdirSync(scratch), ['keep']);
  assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'sentinel');
});

for (const invalidDir of ['missing', 'file']) {
  test(`${invalidDir} TMPDIR fails clearly before starting the smoke test`, (t) => {
    const root = mkdtempSync(join(tmpdir(), 'hub-smoke-test-'));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const guard = smokeCommandGuard(root);
    const invalidPath = invalidDir === 'missing' ? join(root, 'missing', 'nested') : join(root, 'file');
    if (invalidDir === 'file') writeFileSync(invalidPath, 'keep');
    const result = spawnSync('bash', [script], {
      env: { ...process.env, ...guard, TMPDIR: invalidPath },
      encoding: 'utf8',
    });

    assert.equal(result.error, undefined);
    assert.ok(Number.isInteger(result.status) && result.status !== 0);
    assert.equal(result.signal, null);
    assert.match(result.stderr, /error: hub-smoke could not create temporary home/);
    assert.equal(result.stdout, '');
    assert.equal(existsSync(guard.SMOKE_COMMAND_MARKER), false);
    assert.deepEqual(readdirSync(root).sort(), invalidDir === 'file' ? ['file', 'guard.sh'] : ['guard.sh']);
  });
}

for (const [template, message] of [
  ['locus-hub-smoke.XXXXXX', 'temporary home'],
  ['locus-hub-proj.XXXXXX', 'temporary project directory'],
  ['locus-hub-smoke-err.XXXXXX', 'temporary error file'],
]) {
  test(`failure creating ${message} stops and cleans up`, (t) => {
    const root = mkdtempSync(join(tmpdir(), 'hub-smoke-test-'));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const bin = join(root, 'bin');
    const scratch = join(root, 'scratch');
    mkdirSync(bin);
    mkdirSync(scratch);
    const guard = smokeCommandGuard(root);
    writeFileSync(join(bin, 'mktemp'), `#!/usr/bin/env bash
for arg in "$@"; do
  if [[ "$arg" == */"$FAIL_TEMPLATE" ]]; then exit 1; fi
done
exec "$REAL_MKTEMP" "$@"
`, { mode: 0o755 });
    const result = spawnSync('bash', [script], {
      env: { ...process.env, ...guard, TMPDIR: scratch, PATH: `${bin}:${process.env.PATH}`, FAIL_TEMPLATE: template, REAL_MKTEMP: realMktemp },
      encoding: 'utf8',
    });

    assert.equal(result.error, undefined);
    assert.ok(Number.isInteger(result.status) && result.status !== 0);
    assert.equal(result.signal, null);
    assert.match(result.stderr, new RegExp(`error: hub-smoke could not create ${message}`));
    assert.equal(result.stdout, '');
    assert.equal(existsSync(guard.SMOKE_COMMAND_MARKER), false);
    assert.deepEqual(readdirSync(scratch), []);
  });
}

for (const [name, scratchName] of [
  ['init failure cleans the used error file and other temporary resources', 'scratch'],
  ['init failure cleans resources under a TMPDIR with spaces and shell metacharacters',
    "scratch with spaces [*] 'quoted'"],
]) test(name, (t) => {
  const root = mkdtempSync(join(tmpdir(), 'hub-smoke-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, scratchName);
  const guard = join(root, 'guard.sh');
  const marker = join(root, 'init-attempts');
  const resources = join(root, 'resources');
  const fixtureScript = join(root, 'scripts', 'hub-smoke.sh');
  mkdirSync(scratch);
  mkdirSync(dirname(fixtureScript));
  mkdirSync(join(root, 'target', 'debug'), { recursive: true });
  copyFileSync(script, fixtureScript);
  // The script checks this path even when locus is supplied by a function.
  writeFileSync(join(root, 'target', 'debug', 'locus'), '#!/usr/bin/env bash\nexit 1\n', { mode: 0o755 });
  writeFileSync(join(scratch, 'unrelated'), 'keep');
  // Intercept even a locally built binary, and fail only after the error file
  // has been created and opened for the init command's stderr.
  writeFileSync(guard, `jq() { return 0; }
sleep() { :; }
locus() {
  if [[ "$1" == init ]]; then
    [[ -d "$SMOKE_HOME" && "$SMOKE_HOME" == "$TMPDIR"/locus-hub-smoke.* ]] || return 2
    [[ -d "$SMOKE_PROJ" && "$SMOKE_PROJ" == "$TMPDIR"/locus-hub-proj.* ]] || return 2
    [[ -f "$SMOKE_ERR" && "$SMOKE_ERR" == "$TMPDIR"/locus-hub-smoke-err.* ]] || return 2
    printf '%s\\n' "$SMOKE_HOME" "$SMOKE_PROJ" "$SMOKE_ERR" > "$RESOURCE_RECORD"
    printf 'attempt\\n' >> "$INIT_MARKER"
    printf 'synthetic init failure\\n' >&2
    return 1
  fi
  return 0
}
`);
  const result = spawnSync('bash', [fixtureScript], {
    env: { ...process.env, BASH_ENV: guard, TMPDIR: scratch, INIT_MARKER: marker, RESOURCE_RECORD: resources },
    encoding: 'utf8',
    timeout: 15_000,
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /warn: hub-smoke init\/pin attempt 1 failed; retrying…/);
  assert.match(result.stderr, /warn: hub-smoke init\/pin attempt 2 failed; retrying…/);
  assert.match(result.stderr, /error: hub-smoke init\/pin failed after 3 attempts/);
  assert.doesNotMatch(result.stdout, /== hub-smoke (OK|FAILED) ==/);
  assert.match(result.stderr, /synthetic init failure/);
  assert.equal(readFileSync(marker, 'utf8'), 'attempt\nattempt\nattempt\n');
  const paths = readFileSync(resources, 'utf8').trim().split('\n');
  assert.equal(paths.length, 3);
  assert.equal(new Set(paths).size, 3);
  for (const path of paths) {
    assert.equal(dirname(path), scratch);
    assert.equal(existsSync(path), false, `${path} must be removed after init failure`);
  }
  assert.deepEqual(readdirSync(scratch), ['unrelated']);
  assert.equal(readFileSync(join(scratch, 'unrelated'), 'utf8'), 'keep');
});
