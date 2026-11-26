// Run: node --test scripts/hub-integration-stubbed-cleanup.test.mjs
// Hermetic: runs hub-integration-test.sh against stub locus/node/jq/cargo
// functions, so no build, broker or network is needed.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-integration-test.sh');

test('first temporary directory allocation failure stops integration commands', () => {
  const result = spawnSync('bash', [join(dirname(script), 'tests/hub-integration-first-mktemp.sh')]);
  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 0, 'shell regression test must pass');
});

test('second mktemp failure removes the earlier populated home and stops integration commands', (t) => {
  const scratch = mkdtempSync(join(tmpdir(), 'locus-hub-int-project-failure-'));
  t.after(() => rmSync(scratch, { recursive: true, force: true }));
  const tmp = join(scratch, 'temporary directory');
  const record = join(scratch, 'home.record');
  const calls = join(scratch, 'calls');
  const home = join(scratch, 'home');
  mkdirSync(tmp);
  mkdirSync(home);
  const sibling = join(tmp, 'keep');
  writeFileSync(sibling, 'untouched');

  const stub = `
mktemp() {
  # Command substitutions run in subshells; persist the count on disk.
  printf 'mktemp\\n' >> "$CALLS"
  case "$(wc -l < "$CALLS" | tr -d ' ')" in
    1)
      [[ "$#" == 2 && "$1" == -d && "$2" == "$TMPDIR/locus-hub-int.XXXXXX" ]] || return 74
      local created
      created="$(command mktemp "$@")" || return
      printf 'allocated\\n' > "$created/sentinel"
      printf '%s\\n' "$created" > "$RECORD"
      printf '%s\\n' "$created"
      ;;
    2)
      [[ "$#" == 2 && "$1" == -d && "$2" == "$TMPDIR/locus-hub-int-proj.XXXXXX" ]] || return 74
      [[ -d "$(cat "$RECORD")" && -f "$(cat "$RECORD")/sentinel" ]] || return 75
      printf 'injected second mktemp failure\\n' >&2
      return 73
      ;;
    *) return 74 ;;
  esac
}
node() { printf 'node\\n' >> "$CALLS"; }
jq() { printf 'jq\\n' >> "$CALLS"; }
cargo() { printf 'cargo\\n' >> "$CALLS"; }
locus() { printf 'locus\\n' >> "$CALLS"; }
locus-mcp() { printf 'locus-mcp\\n' >> "$CALLS"; }
export -f mktemp node jq cargo locus locus-mcp
exec bash "$1"
`;
  const result = spawnSync('bash', ['-c', stub, 'project-failure', script], {
    env: { ...process.env, HOME: home, TMPDIR: tmp, RECORD: record, CALLS: calls, LOCUS_CONTROL_CAPABILITY: 'synthetic' },
    encoding: 'utf8',
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 73, 'project allocation failure must propagate');
  assert.match(result.stderr, /injected second mktemp failure/);
  assert.equal(result.stdout, '', 'integration output must not start');
  assert.equal(readFileSync(calls, 'utf8'), 'mktemp\nmktemp\n', 'exactly two allocations and no integration commands must run');
  const allocatedHome = readFileSync(record, 'utf8').trim();
  assert.equal(dirname(allocatedHome), tmp);
  assert.equal(existsSync(allocatedHome), false, 'the successfully allocated populated home must be removed');
  assert.deepEqual(readdirSync(home), [], 'the isolated user home must remain untouched');
  assert.deepEqual(readdirSync(tmp), ['keep'], 'cleanup must preserve unrelated files');
  assert.equal(readFileSync(sibling, 'utf8'), 'untouched');
});

// Exported bash functions shadow the real commands in the child script. The
// canned answers satisfy every check; `mcp` fails so the multi-tenant slice
// soft-skips. At init, record both real directories before the EXIT trap runs.
const harness = `
node() { [[ "\${HUB_MINT_JSON:-}" == EXPIRED ]] && return 1; return 0; }
jq() { [[ "$*" == *'has("credential_ref")'* || "$*" == *dogfood-ready.jq* ]] && return 1; return 0; }
cargo() { :; }
locus-mcp() { :; }
sleep() { :; }
locus() {
  case " $* " in
    *" mcp "*) return 1 ;;
    *" status --oneline "*) [[ -f "$LOCUS_HOME/unpinned" ]] && echo unpinned || echo personal:t ;;
    *" leave "*) : > "$LOCUS_HOME/unpinned" ;;
    *" pin "*) rm -f "$LOCUS_HOME/unpinned" ;;
    *" --ttl 1s "*) echo EXPIRED ;;
    *" init "*)
      [[ -d "$SMOKE_HOME" && -d "$SMOKE_PROJ" && "$LOCUS_HOME" == "$SMOKE_HOME" ]] || return 74
      echo seeded > "$LOCUS_HOME/store.txt"
      echo seeded > "$SMOKE_PROJ/fixture.txt"
      printf '%s\\n' "$SMOKE_HOME" "$SMOKE_PROJ" > "$RECORD"
      # Model a scan listing left behind by an interrupted scan; the EXIT trap
      # must remove it even when the rest of the smoke run succeeds.
      : > "$TMPDIR/.locus-hub-int-scan.$$.fixture"
      printf '%s\\n' "$TMPDIR/.locus-hub-int-scan.$$.fixture" > "$SCAN_RECORD"
      ;;
  esac
  return 0
}
export -f node jq cargo locus-mcp sleep locus
exec bash "$1"
`;

test('stubbed successful run removes both temporary directories', (t) => {
  const scratch = mkdtempSync(join(tmpdir(), 'locus-hub-int-stub-'));
  t.after(() => rmSync(scratch, { recursive: true, force: true }));
  const record = join(scratch, 'home.record');
  const scanRecord = join(scratch, 'scan.record');
  const tmp = join(scratch, 'tmp');
  mkdirSync(tmp);

  const result = spawnSync('bash', ['-c', harness, 'stub-cleanup', script], {
    env: { ...process.env, TMPDIR: tmp, RECORD: record, SCAN_RECORD: scanRecord }, encoding: 'utf8',
    maxBuffer: 4 * 1024 * 1024,
  });

  assert.equal(result.status, 0, `${result.stdout}${result.stderr}`);
  assert.match(result.stdout, /^== hub-integration-test OK ==$/m);
  const paths = readFileSync(record, 'utf8').trim().split('\n');
  assert.equal(paths.length, 2, 'both directories must be recorded during the run');
  const [home, project] = paths;
  assert.notEqual(home, project);
  assert.equal(dirname(home), tmp, 'SMOKE_HOME must be allocated beneath TMPDIR');
  assert.equal(existsSync(home), false, 'SMOKE_HOME must be removed on exit');
  assert.equal(dirname(project), tmp, 'SMOKE_PROJ must be allocated beneath TMPDIR');
  assert.equal(existsSync(project), false, 'SMOKE_PROJ must be removed on exit');
  const listing = readFileSync(scanRecord, 'utf8').trim();
  assert.equal(dirname(listing), tmp);
  assert.equal(existsSync(listing), false, 'an outstanding scan listing must be removed on exit');
  assert.deepEqual(readdirSync(tmp), [], 'no temporary file or directory may remain');
});

test('spaced TMPDIR contains both temporary directories and preserves a sibling on cleanup', (t) => {
  const scratch = mkdtempSync(join(tmpdir(), 'locus-hub-int-spaced-'));
  t.after(() => rmSync(scratch, { recursive: true, force: true }));
  const tmp = join(scratch, 'temporary directory with spaces');
  const record = join(scratch, 'home.record');
  const scanRecord = join(scratch, 'scan.record');
  mkdirSync(tmp);
  const sibling = join(tmp, 'existing sibling.txt');
  writeFileSync(sibling, 'keep this file\n');

  const result = spawnSync('bash', ['-c', harness, 'stub-cleanup', script], {
    env: { ...process.env, TMPDIR: tmp, RECORD: record, SCAN_RECORD: scanRecord }, encoding: 'utf8',
    maxBuffer: 4 * 1024 * 1024,
  });

  assert.equal(result.status, 0, `${result.stdout}${result.stderr}`);
  assert.match(result.stdout, /^== hub-integration-test OK ==$/m);
  const paths = readFileSync(record, 'utf8').trim().split('\n');
  assert.equal(paths.length, 2, 'both directories must be recorded during the run');
  const [home, project] = paths;
  assert.notEqual(home, project);
  for (const path of [home, project]) {
    assert.equal(dirname(path), tmp, 'temporary directory must be directly beneath TMPDIR');
    assert.equal(existsSync(path), false, 'temporary directory must be removed on exit');
  }
  assert.deepEqual(readdirSync(tmp), ['existing sibling.txt']);
  assert.equal(existsSync(readFileSync(scanRecord, 'utf8').trim()), false,
    'an outstanding scan listing must be removed from a spaced TMPDIR');
  assert.equal(readFileSync(sibling, 'utf8'), 'keep this file\n');
});
