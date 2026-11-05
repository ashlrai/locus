// Run: node --test scripts/hub-integration-stubbed-cleanup.test.mjs
// Hermetic: runs hub-integration-test.sh against stub locus/node/jq/cargo
// functions, so no build, broker or network is needed.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-integration-test.sh');

// Exported bash functions shadow the real commands in the child script. The
// canned answers satisfy every check; `mcp` fails so the multi-tenant slice
// soft-skips. The final `locus` call lists SMOKE_HOME/SMOKE_PROJ for the test.
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
    *" init "*) echo seeded > "$LOCUS_HOME/store.txt"; printf '%s\\n' "$LOCUS_HOME" > "$RECORD" ;;
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
  const tmp = join(scratch, 'tmp');
  mkdirSync(tmp);

  const result = spawnSync('bash', ['-c', harness, 'stub-cleanup', script], {
    env: { ...process.env, TMPDIR: tmp, RECORD: record }, encoding: 'utf8',
    maxBuffer: 4 * 1024 * 1024,
  });

  assert.equal(result.status, 0, `${result.stdout}${result.stderr}`);
  assert.match(result.stdout, /^== hub-integration-test OK ==$/m);
  const home = readFileSync(record, 'utf8').trim();
  assert.equal(dirname(home), tmp, 'SMOKE_HOME must be allocated beneath TMPDIR');
  assert.equal(existsSync(home), false, 'SMOKE_HOME must be removed on exit');
  const project = /ambient canary absent under (\S+locus-hub-int-proj\.\w+)/.exec(result.stdout)?.[1];
  assert.ok(project, 'the run must scan the smoke project');
  assert.equal(dirname(project), tmp, 'SMOKE_PROJ must be allocated beneath TMPDIR');
  assert.equal(existsSync(project), false, 'SMOKE_PROJ must be removed on exit');
  assert.deepEqual(readdirSync(tmp), [], 'no temporary file or directory may remain');
});
