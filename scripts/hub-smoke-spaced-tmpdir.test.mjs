// Run: node --test scripts/hub-smoke-spaced-tmpdir.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-smoke.sh');

test('smoke preserves a spaced, glob-like TMPDIR and cleans its own resources', (t) => {
  const root = mkdtempSync('/tmp/lhs-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'a [*] b');
  const record = join(root, 'created');
  const marker = join(root, 'resources-observed');
  const hook = join(root, 'hook.sh');
  mkdirSync(scratch);
  writeFileSync(join(scratch, 'locus-hub-smoke.unrelated'), 'keep');
  writeFileSync(hook, `mktemp() {
  case "\${1:-}" in
    -d) [[ $# -eq 2 && "$2" == "$TMPDIR"/locus-hub-*.XXXXXX ]] || return 71 ;;
    *) [[ $# -eq 1 && "$1" == "$TMPDIR"/locus-hub-*.XXXXXX ]] || return 71 ;;
  esac
  local path
  path="$(command mktemp "$@")" || return
  printf '%s\\n' "$path" >> "$SMOKE_RECORD"
  printf '%s\\n' "$path"
}
cargo() { return 0; }
jq() { printf '%s\\n' grant; }
locus() {
  if [[ "\${1:-}" == init ]]; then
    local path count=0
    while IFS= read -r path; do
      [[ -e "$path" && "\${path%/*}" == "$TMPDIR" ]] || return 72
      count=$((count + 1))
    done < "$SMOKE_RECORD"
    [[ $count -eq 3 ]] || return 72
    : > "$SMOKE_MARKER"
  fi
  if [[ "\${1:-}" == status && "\${2:-}" == --oneline ]]; then
    printf '%s\\n' personal:tenant
  else
    printf '%s\\n' '{}'
  fi
}
`);
  const result = spawnSync('bash', [script], {
    env: { ...process.env, BASH_ENV: hook, TMPDIR: scratch, SMOKE_RECORD: record, SMOKE_MARKER: marker },
    encoding: 'utf8',
  });
  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 0);
  assert.match(result.stdout, /^== hub-smoke OK ==$/m);
  assert.doesNotMatch(result.stdout, /^skip\s/m);
  assert.ok(existsSync(marker), 'all resources must exist during initialization');
  const paths = readFileSync(record, 'utf8').trim().split('\n');
  assert.equal(paths.length, 3);
  for (const [index, prefix] of ['locus-hub-smoke.', 'locus-hub-proj.', 'locus-hub-smoke-err.'].entries()) {
    assert.equal(dirname(paths[index]), scratch);
    assert.ok(paths[index].slice(scratch.length + 1).startsWith(prefix));
    assert.equal(existsSync(paths[index]), false);
  }
  assert.deepEqual(readdirSync(scratch), ['locus-hub-smoke.unrelated']);
  assert.equal(readFileSync(join(scratch, 'locus-hub-smoke.unrelated'), 'utf8'), 'keep');
});
