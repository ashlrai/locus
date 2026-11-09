// Run: node --test scripts/hub-smoke-stubbed-cleanup.test.mjs
// Hermetic: runs a copy of hub-smoke.sh against a stub `locus`, so no build or broker is needed.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import {
  chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync,
} from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-smoke.sh');

const stub = `#!/usr/bin/env bash
case "$1 $2" in
  "agent report")
    [[ "\${3:-}" == "--json" ]] || exit 0
    if [[ -f "$LOCUS_HOME/pinned" ]]; then
      echo '{"status":"ready","ready":true,"exit_code":0,"status_oneline":"personal:t","mcp_command":"locus-mcp","required_servers":["locus","phantom"],"mcp_registered":true,"doctor":{"home":"h","verdict":"SAFE"},"commands":[],"home":"h"}'
    else
      echo '{"status":"protected","ready":false,"exit_code":1,"status_oneline":"unpinned","mcp_command":"locus-mcp","required_servers":["locus","phantom"],"mcp_registered":true,"doctor":{"home":"h","verdict":"SAFE"},"commands":[],"home":"h"}'
    fi ;;
  "pin personal") touch "$LOCUS_HOME/pinned" ;;
  "leave "*|"leave ") rm -f "$LOCUS_HOME/pinned" ;;
  "doctor --json") echo '{"version":"x","home":"h","seal_ok":true,"bindings":[],"runtime":{},"approvals":{},"pending_approvals":0,"dual_control_waiting":0,"phantom_on_path":false,"unresolved_phm":[],"autopin":{},"workspace":{},"audit":{},"findings":[],"issues":[],"verdict":"SAFE","ok":true}' ;;
  "whoami --json") echo '{"session_id":"s","binding_alias":"personal","tenant":"t","providers":[],"seal_ok":true,"mode":"m"}' ;;
  "status --json") echo '{"pinned":true,"binding":"personal","tenant":"t","session_id":"s"}' ;;
  "status --oneline")
    errors=("$TMPDIR"/locus-hub-smoke-err.*)
    [[ -d "$LOCUS_HOME" && -f .mcp.json && \${#errors[@]} -eq 1 && -f "\${errors[0]}" ]] || exit 1
    mkdir -p "$LOCUS_HOME/nested" nested
    touch "$LOCUS_HOME/nested/content" nested/content
    printf '%s\\n' "$LOCUS_HOME" "$PWD" "\${errors[0]}" > "$RESOURCE_LOG"
    echo 'personal:t' ;;
  "mcp mint") exit 1 ;;
esac
exit 0
`;

test('stubbed successful run preserves spaced temporary paths and removes only its resources', (t) => {
  // Short path: keeps any socket path small and the listing below easy to read.
  const tmp = mkdtempSync('/tmp/locus-stub-test-');
  t.after(() => rmSync(tmp, { recursive: true, force: true }));
  const fakeRoot = join(tmp, 'repo');
  const scratch = join(tmp, 'scratch with spaces');
  const hook = join(tmp, 'hook');
  const calls = join(tmp, 'calls');
  mkdirSync(join(fakeRoot, 'scripts'), { recursive: true });
  mkdirSync(join(fakeRoot, 'target', 'debug'), { recursive: true });
  mkdirSync(join(fakeRoot, 'schema'));
  mkdirSync(join(fakeRoot, 'docs'));
  mkdirSync(join(fakeRoot, 'integrations', 'phantom'), { recursive: true });
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'unrelated'), 'keep');
  writeFileSync(hook, `
mktemp() {
  case "$1" in
    -d) [[ $# -eq 2 && "\${2%/*}" == "$TMPDIR" ]] || return 91 ;;
    *)  [[ $# -eq 1 && "\${1%/*}" == "$TMPDIR" ]] || return 91 ;;
  esac
  printf 'mktemp\\n' >> "$CALLS"
  command mktemp "$@"
}
rm() {
  if [[ "\${1:-}" == -rf ]]; then
    [[ $# -eq 5 && "$2" == -- && "\${3%/*}" == "$TMPDIR" &&
       "\${4%/*}" == "$TMPDIR" && "\${5%/*}" == "$TMPDIR" ]] || return 92
    printf 'cleanup\\n' >> "$CALLS"
  fi
  command rm "$@"
}
`);
  copyFileSync(script, join(fakeRoot, 'scripts', 'hub-smoke.sh'));
  const bin = join(fakeRoot, 'target', 'debug', 'locus');
  writeFileSync(bin, stub);
  chmodSync(bin, 0o755);
  for (const f of ['agent-report', 'doctor', 'hub-gate', 'mcp-grant']) {
    writeFileSync(join(fakeRoot, 'schema', `${f}.schema.json`), '{}');
  }
  for (const f of ['docs/hub-integration.md', 'integrations/phantom/README.md',
    'integrations/phantom/fleet-preflight.md']) {
    writeFileSync(join(fakeRoot, f), '');
  }

  const result = spawnSync('bash', [join(fakeRoot, 'scripts', 'hub-smoke.sh')], {
    env: { ...process.env, BASH_ENV: hook, TMPDIR: scratch, CALLS: calls,
      RESOURCE_LOG: join(tmp, 'resources') }, encoding: 'utf8',
  });

  assert.equal(result.status, 0, `${result.stdout}${result.stderr}`);
  assert.ok(result.stdout.includes('== hub-smoke OK =='));
  assert.equal(readFileSync(calls, 'utf8'), 'mktemp\nmktemp\nmktemp\ncleanup\n');
  const resources = readFileSync(join(tmp, 'resources'), 'utf8').trimEnd().split('\n');
  assert.equal(resources.length, 3);
  for (const [index, prefix] of ['locus-hub-smoke.', 'locus-hub-proj.', 'locus-hub-smoke-err.'].entries()) {
    assert.equal(dirname(resources[index]), scratch);
    assert.ok(resources[index].slice(scratch.length + 1).startsWith(prefix));
    assert.equal(existsSync(resources[index]), false, `${prefix} resource must be removed`);
  }
  assert.deepEqual(readdirSync(scratch), ['unrelated']);
  assert.equal(readFileSync(join(scratch, 'unrelated'), 'utf8'), 'keep');
  assert.equal(existsSync(scratch), true);
});

test('early command failure removes every allocated temporary path', (t) => {
  const tmp = mkdtempSync('/tmp/locus-stub-failure-');
  t.after(() => rmSync(tmp, { recursive: true, force: true }));
  const fakeRoot = join(tmp, 'repo');
  const scratch = join(tmp, 'scratch with spaces');
  const bin = join(tmp, 'bin');
  const home = join(tmp, 'home');
  const record = join(tmp, 'allocations');
  mkdirSync(join(fakeRoot, 'scripts'), { recursive: true });
  mkdirSync(bin);
  // Throwaway HOME keeps the real ~/.cargo/bin off the script's PATH prepend,
  // so the cargo stub below is the one the script finds.
  mkdirSync(home);
  mkdirSync(scratch);
  mkdirSync(join(fakeRoot, 'target', 'debug'), { recursive: true });
  copyFileSync(script, join(fakeRoot, 'scripts', 'hub-smoke.sh'));
  writeFileSync(join(fakeRoot, 'target', 'debug', 'locus'), '#!/usr/bin/env bash\nexit 1\n', { mode: 0o755 });
  writeFileSync(join(scratch, 'keep'), 'sentinel');

  const lookup = spawnSync('which', ['mktemp'], { encoding: 'utf8' });
  assert.equal(lookup.status, 0);
  writeFileSync(join(bin, 'mktemp'), `#!/usr/bin/env bash
path="$("$REAL_MKTEMP" "$@")" || exit 1
printf '%s\\n' "$path" >> "$ALLOCATIONS"
printf '%s\\n' "$path"
`, { mode: 0o755 });
  // A failed help probe triggers this build, after all allocations.
  writeFileSync(join(bin, 'cargo'), `#!/usr/bin/env bash
[[ $(wc -l < "$ALLOCATIONS") -eq 3 ]] || exit 98
while IFS= read -r path; do [[ -e "$path" ]] || exit 98; done < "$ALLOCATIONS"
exit 42
`, { mode: 0o755 });

  const env = { ...process.env, BASH_ENV: '', HOME: home, TMPDIR: scratch,
    PATH: `${bin}:${process.env.PATH}`, REAL_MKTEMP: lookup.stdout.trim(), ALLOCATIONS: record,
    LOCUS_CONTROL_CAPABILITY: 'synthetic-test-capability' };
  const result = spawnSync('bash', [join(fakeRoot, 'scripts', 'hub-smoke.sh')], {
    env, encoding: 'utf8',
  });

  assert.equal(result.status, 42);
  assert.equal(result.signal, null);
  const paths = readFileSync(record, 'utf8').trim().split('\n');
  assert.equal(paths.length, 3);
  for (const [index, prefix] of ['locus-hub-smoke.', 'locus-hub-proj.', 'locus-hub-smoke-err.'].entries()) {
    assert.equal(dirname(paths[index]), scratch);
    assert.ok(paths[index].slice(scratch.length + 1).startsWith(prefix));
    assert.equal(existsSync(paths[index]), false, `allocation ${index} must be removed`);
  }
  assert.deepEqual(readdirSync(scratch), ['keep']);
  assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'sentinel');
});
