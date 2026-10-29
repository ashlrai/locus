// Run: node --test scripts/dogfood.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'dogfood.sh');
const harness = String.raw`
source "$1"
locus() {
  local stage="$1"
  [[ "$stage" != mcp ]] || stage="$2"
  printf '%s\n' "$stage" >> "$TRACE_FILE"
  if [[ "$stage" == init ]]; then
    printf '%s\n' "$PWD" > "$PROBE_FILE"
    mkdir -p "$LOCUS_HOME"
    touch "$LOCUS_HOME/sentinel"
  fi
  [[ "$stage" != "$FAIL_STAGE" ]] || return 1
  case "$stage" in
    mint) printf '%s\n' '{"grant_id":"grant_fixture","token":"lmt_fixture"}' ;;
    list)
      if [[ "$FAIL_STAGE" == http ]]; then
        printf '%s\n' '[{"grant_id":"grant_fixture","revoked":false,"expired":false}]'
      else
        printf '%s\n' '[]'
      fi ;;
  esac
}
curl() {
  if [[ "$*" == */health ]]; then
    for _ in {1..100}; do
      [[ -f "$PID_FILE" ]] && return 0
      /bin/sleep 0.01
    done
    return 1
  fi
  printf '500'
}
# Match the production caller: errexit is disabled within the function.
mt_grant_probe || exit 42
exit 0
`;

for (const [failure, expectedStages] of [
  ['init', ['init']],
  ['binding', ['init', 'binding']],
  ['mint', ['init', 'binding', 'mint']],
  ['roster', ['init', 'binding', 'mint', 'list']],
  ['http', ['init', 'binding', 'mint', 'list']],
]) {
  test(`multi-tenant probe cleans its temporary directory after ${failure} failure`, (t) => {
    const root = mkdtempSync(join(tmpdir(), 'locus-dogfood-test-'));
    const pidFile = join(root, 'server-pid');
    let serverStopped = false;
    t.after(() => {
      if (!serverStopped && existsSync(pidFile)) {
        try {
          process.kill(Number(readFileSync(pidFile, 'utf8').trim()));
        } catch (error) {
          if (error.code !== 'ESRCH') throw error;
        }
      }
      rmSync(root, { recursive: true, force: true });
    });
    const scratch = join(root, 'scratch');
    mkdirSync(scratch);
    writeFileSync(join(scratch, 'keep'), 'preserve');
    const trace = join(root, 'trace');
    const probe = join(root, 'probe');
    const server = join(root, 'server');
    writeFileSync(server, '#!/usr/bin/env bash\nprintf "%s\\n" "$$" > "$PID_FILE"\nexec sleep 120\n', { mode: 0o755 });
    const result = spawnSync('bash', ['-c', harness, 'dogfood-test', script], {
      cwd: root,
      env: { ...process.env, TMPDIR: scratch, TRACE_FILE: trace, PROBE_FILE: probe, FAIL_STAGE: failure, MCP_BIN: server, PID_FILE: pidFile },
      encoding: 'utf8',
    });

    assert.equal(result.signal, null);
    assert.equal(result.status, 42, result.stderr);
    assert.equal(result.stdout, '');
    assert.deepEqual(readFileSync(trace, 'utf8').trim().split('\n'), expectedStages);
    const probeRoot = readFileSync(probe, 'utf8').trim();
    assert.match(probeRoot, /\/locus-mt-dogfood\.[^/]+$/);
    assert.equal(dirname(probeRoot), scratch);
    assert.equal(existsSync(probeRoot), false);
    assert.deepEqual(readdirSync(scratch), ['keep']);
    assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'preserve');
    if (failure === 'http') {
      const pid = Number(readFileSync(pidFile, 'utf8').trim());
      assert.ok(Number.isInteger(pid) && pid > 0);
      assert.throws(() => process.kill(pid, 0), { code: 'ESRCH' });
      serverStopped = true;
    }
  });
}
