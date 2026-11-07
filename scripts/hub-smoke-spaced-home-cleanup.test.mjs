// Run: node --test scripts/hub-smoke-spaced-home-cleanup.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-smoke.sh');

// Stubs: init records LOCUS_HOME, then fails (or succeeds) on demand.
function run(t, initExit) {
  const root = mkdtempSync('/tmp/lhs-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'my temp dir');
  const record = join(root, 'home-seen');
  const hook = join(root, 'hook.sh');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'sibling'), 'keep');
  writeFileSync(hook, `cargo() { return 0; }
sleep() { :; }
jq() { cat >/dev/null; printf '%s\\n' grant; }
locus() {
  case "\${1:-}" in
    init) printf '%s' "$LOCUS_HOME" > "$HOME_RECORD"; return ${initExit} ;;
    status) [[ "\${2:-}" == --oneline ]] && { printf '%s\\n' personal:tenant; return; } ;;
  esac
  printf '%s\\n' '{}'
}
`);
  const result = spawnSync('bash', [script], {
    env: { ...process.env, BASH_ENV: hook, TMPDIR: scratch, HOME_RECORD: record },
    encoding: 'utf8',
    timeout: 15_000,
  });
  assert.equal(result.error, undefined);
  return { result, scratch, home: readFileSync(record, 'utf8') };
}

for (const [label, initExit, status] of [['success', 0, 0], ['init failure', 1, 1]]) {
  test(`SMOKE_HOME under a spaced TMPDIR is removed on ${label}; parent and siblings survive`, (t) => {
    const { result, scratch, home } = run(t, initExit);
    assert.equal(result.status, status, result.stderr);
    assert.equal(dirname(home), scratch, 'created directly under the exact spaced parent');
    assert.equal(existsSync(home), false, 'SMOKE_HOME must be removed on exit');
    assert.equal(existsSync(scratch), true, 'parent TMPDIR must survive');
    assert.deepEqual(readdirSync(scratch), ['sibling'], 'only the sibling remains');
  });
}
