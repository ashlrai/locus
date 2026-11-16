// Run: node --test scripts/e2e-workspace-interruption.test.mjs
// An interrupted e2e run must remove the workspace it created and nothing
// else: the parent temporary directory and unrelated siblings stay intact.
// The shell below replays scripts/e2e.sh's own create_workspace, e2e_cleanup
// and EXIT trap, then blocks in a builtin so a signal lands mid-run.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { setTimeout as delay } from 'node:timers/promises';
import { test } from 'node:test';

const source = readFileSync(join(dirname(fileURLToPath(import.meta.url)), 'e2e.sh'), 'utf8');

function extract(name) {
  const body = source.match(new RegExp(`^${name}\\(\\) \\{[\\s\\S]*?^\\}`, 'm'))?.[0];
  assert.ok(body, `e2e.sh must define ${name}`);
  return body;
}

const trapLine = source.match(/^trap 'e2e_cleanup' EXIT$/m)?.[0];

test('e2e.sh installs the cleanup trap on EXIT', () => {
  assert.ok(trapLine, "e2e.sh must keep trap 'e2e_cleanup' EXIT");
});

for (const signal of ['SIGINT', 'SIGTERM']) {
  for (const [mode, scratchName] of [['plain', 'scratch'], ['spaced', 'scratch dir [*]']]) {
    test(`${signal} removes the created workspace and spares its parent (${mode} TMPDIR)`, { timeout: 15_000 }, async (t) => {
      const root = mkdtempSync(join(tmpdir(), 'locus-e2e-ws-interrupt-'));
      t.after(() => rmSync(root, { recursive: true, force: true }));
      const scratch = join(root, scratchName);
      mkdirSync(scratch);
      writeFileSync(join(scratch, 'keep'), 'unrelated');
      mkdirSync(join(scratch, 'locus-ws.unrelated'));
      const record = join(root, 'workspace');

      const child = spawn('bash', ['-c', `set -euo pipefail
LOCUS_HOME="$(mktemp -d "$TMPDIR/locus-e2e.XXXXXX")"
HTTP_MCP_PID=""; MT_MCP_PID=""; WS_DIR=""; GRAPH_HOME=""; SECURITY_HOME=""; SECURITY_WS=""
${extract('create_workspace')}
${extract('e2e_cleanup')}
${trapLine}
create_workspace
touch "$WS_DIR/fixture"
printf '%s' "$WS_DIR" > "$RECORD"
read -r ignored
`], { env: { ...process.env, TMPDIR: scratch, RECORD: record }, stdio: ['pipe', 'ignore', 'ignore'] });
      t.after(() => { if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL'); });
      const exited = new Promise((resolve, reject) => {
        child.once('error', reject);
        child.once('exit', (code, sig) => resolve({ code, sig }));
      });

      for (let i = 0; i < 250 && !existsSync(record); i++) await delay(20);
      assert.ok(existsSync(record), 'script did not reach the interruption point');
      const workspace = readFileSync(record, 'utf8');
      assert.ok(existsSync(join(workspace, 'fixture')));

      assert.ok(child.kill(signal));
      await exited;

      assert.equal(existsSync(workspace), false, 'interrupted run must remove its workspace');
      assert.ok(existsSync(scratch), 'cleanup must not remove the parent directory');
      assert.deepEqual(readdirSync(scratch).sort(), ['keep', 'locus-ws.unrelated'],
        'only the run-owned directories may be removed');
      assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'unrelated');
    });
  }
}
