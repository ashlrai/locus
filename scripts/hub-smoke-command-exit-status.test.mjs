// Run: node --test scripts/hub-smoke-command-exit-status.test.mjs
// Hermetic: a smoke command exits nonzero after init/pin and the checks have
// populated every temporary resource; the script must pass that exit code
// through unchanged and remove all three resources.
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-smoke.sh');
const realMktemp = execFileSync('which', ['mktemp'], { encoding: 'utf8' }).trim();
const EXIT_CODE = 37; // distinct from 1, so a swallowed or remapped status is visible

test('a smoke command exiting nonzero propagates its status and removes every temporary path', (t) => {
  const root = mkdtempSync('/tmp/lhs-exit-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const bin = join(root, 'bin');
  const scratch = join(root, 'temporary files with spaces');
  const record = join(root, 'allocations');
  const hook = join(root, 'hook.sh');
  mkdirSync(bin);
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'keep'), 'sentinel');
  writeFileSync(join(bin, 'mktemp'), `#!/usr/bin/env bash
path="$("$REAL_MKTEMP" "$@")" || exit 1
printf '%s\\n' "$path" >> "$ALLOCATIONS"
printf '%s\\n' "$path"
`, { mode: 0o755 });
  // Everything passes until \`status --oneline\`, which fails with EXIT_CODE
  // once init/pin have written into SMOKE_HOME and SMOKE_ERR.
  writeFileSync(hook, `jq() { cat >/dev/null; }
cargo() { return 0; }
sleep() { :; }
locus() {
  case "\${1:-} \${2:-}" in
    "init "*) printf 'seeded\\n' > "$LOCUS_HOME/store.txt"; return 0 ;;
    "status --oneline") printf 'synthetic status failure\\n' >&2; return ${EXIT_CODE} ;;
  esac
  printf '%s\\n' '{}'
}
`);

  const result = spawnSync('bash', [script], {
    env: { ...process.env, BASH_ENV: hook, TMPDIR: scratch, PATH: `${bin}:${process.env.PATH}`,
      REAL_MKTEMP: realMktemp, ALLOCATIONS: record },
    encoding: 'utf8',
    timeout: 15_000,
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, EXIT_CODE, `${result.stdout}${result.stderr}`);
  assert.match(result.stderr, /synthetic status failure/);
  assert.ok(result.stdout.includes('ok    agent report --json'), 'checks before the failure must have run');
  assert.doesNotMatch(result.stdout, /== hub-smoke (OK|FAILED) ==/);

  const paths = readFileSync(record, 'utf8').trim().split('\n');
  assert.equal(paths.length, 3);
  for (const [index, prefix] of ['locus-hub-smoke.', 'locus-hub-proj.', 'locus-hub-smoke-err.'].entries()) {
    assert.equal(dirname(paths[index]), scratch);
    assert.ok(paths[index].slice(scratch.length + 1).startsWith(prefix));
    assert.equal(existsSync(paths[index]), false, `${prefix} must be removed`);
  }
  assert.deepEqual(readdirSync(scratch), ['keep']);
  assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'sentinel');
});
