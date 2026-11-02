// Run: node --test scripts/hub-smoke-home-cleanup.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-smoke.sh');

test('normal completion removes the populated temporary home', (t) => {
  // Keep the path short enough for the authority broker's Unix socket.
  const root = mkdtempSync('/tmp/locus-home-test-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const homeRecord = join(root, 'home');
  const observed = join(root, 'observed');
  const hook = join(root, 'hook.sh');
  writeFileSync(join(root, 'keep'), 'sentinel');
  writeFileSync(hook, `mktemp() {
  local path
  path="$(command mktemp "$@")" || return
  if [[ "\${!#}" == */locus-hub-smoke.XXXXXX ]]; then
    mkdir "$path/nested" || return
    printf '%s' sentinel > "$path/nested/cleanup-sentinel" || return
    printf '%s' "$path" > "$HOME_RECORD"
  fi
  printf '%s\\n' "$path"
}
rm() {
  local home arg targeted=
  home="$(cat "$HOME_RECORD")"
  for arg in "$@"; do
    [[ "$arg" == "$home" ]] && targeted=1
  done
  if [[ -n "$targeted" && "\${LOCUS_HOME:-}" == "$home" && -d "$home/bindings" \\
        && -f "$home/daemon.key" && -f "$home/nested/cleanup-sentinel" \\
        && "$(cat "$home/nested/cleanup-sentinel")" == sentinel ]]; then
    printf '%s' observed > "$HOME_OBSERVED"
  fi
  command rm "$@"
}
`);
  const result = spawnSync('bash', [script], {
    env: { ...process.env, TMPDIR: root, BASH_ENV: hook,
      HOME_RECORD: homeRecord, HOME_OBSERVED: observed },
    encoding: 'utf8',
    maxBuffer: 4 * 1024 * 1024,
  });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 0, 'smoke must complete successfully');
  assert.ok(result.stdout.split('\n').includes('== hub-smoke OK =='), 'success banner missing');
  assert.doesNotMatch(result.stdout, /^skip\s/m, 'all smoke checks must run');
  const home = readFileSync(homeRecord, 'utf8');
  assert.equal(dirname(home), root);
  assert.ok(home.slice(root.length + 1).startsWith('locus-hub-smoke.'));
  assert.ok(result.stdout.includes(`== hub-smoke LOCUS_HOME=${home} ==`),
    'the recorded directory must be the exported LOCUS_HOME');
  assert.equal(readFileSync(observed, 'utf8'), 'observed',
    'cleanup must target the active, populated Locus home and its nested sentinel');
  assert.equal(existsSync(home), false, 'temporary home must be removed');
  assert.equal(readFileSync(join(root, 'keep'), 'utf8'), 'sentinel');
});
