// Run: node --test scripts/hub-smoke-stubbed-cleanup.test.mjs
// Hermetic: runs a copy of hub-smoke.sh against a stub `locus`, so no build or broker is needed.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import {
  chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readdirSync, rmSync, writeFileSync,
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
  "status --oneline") echo 'personal:t' ;;
  "mcp mint") exit 1 ;;
esac
exit 0
`;

test('stubbed successful run leaves no temporary project directory', (t) => {
  // Short path: keeps any socket path small and the listing below easy to read.
  const tmp = mkdtempSync('/tmp/locus-stub-test-');
  t.after(() => rmSync(tmp, { recursive: true, force: true }));
  const fakeRoot = join(tmp, 'repo');
  const scratch = join(tmp, 'scratch');
  mkdirSync(join(fakeRoot, 'scripts'), { recursive: true });
  mkdirSync(join(fakeRoot, 'target', 'debug'), { recursive: true });
  mkdirSync(join(fakeRoot, 'schema'));
  mkdirSync(join(fakeRoot, 'docs'));
  mkdirSync(join(fakeRoot, 'integrations', 'phantom'), { recursive: true });
  mkdirSync(scratch);
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
    env: { ...process.env, TMPDIR: scratch }, encoding: 'utf8',
  });

  assert.equal(result.status, 0, `${result.stdout}${result.stderr}`);
  assert.ok(result.stdout.includes('== hub-smoke OK =='));
  assert.deepEqual(readdirSync(scratch), [], 'no temp project, home or error file may remain');
  assert.equal(existsSync(scratch), true);
});
