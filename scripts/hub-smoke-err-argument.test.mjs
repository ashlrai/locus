// Run: node --test scripts/hub-smoke-err-argument.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { test } from 'node:test';
import { dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-smoke.sh');

test('SMOKE_ERR with a space-containing TMPDIR is passed as one whole argument', (t) => {
  const root = mkdtempSync('/tmp/lhs-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'temp dir with spaces');
  const record = join(root, 'argv');
  const hook = join(root, 'hook.sh');
  mkdirSync(scratch);
  // Each wrapper logs "<command> <argc> <last-arg>" so the test can check the
  // path was never word-split. pin always fails, which drives the error file
  // through mktemp, the redirections, head, and the EXIT-trap rm.
  writeFileSync(hook, `mktemp() {
  local last="\${!#}"
  [[ "$last" == "$TMPDIR"/locus-hub-smoke-err.XXXXXX ]] && printf 'mktemp %s %s\\n' "$#" "$last" >> "$SMOKE_ARGV"
  command mktemp "$@"
}
head() { printf 'head %s %s\\n' "$#" "\${!#}" >> "$SMOKE_ARGV"; command head "$@"; }
rm() {
  local last="\${!#}"
  [[ "$last" == "$TMPDIR"/locus-hub-smoke-err.* ]] && printf 'rm %s %s\\n' "$#" "$last" >> "$SMOKE_ARGV"
  command rm "$@"
}
cargo() { return 0; }
sleep() { :; }
jq() { cat >/dev/null; }
locus() {
  case "\${1:-}" in
    pin) printf 'synthetic pin diagnostic\\n' >&2; return 1 ;;
  esac
  return 0
}
`);
  const result = spawnSync('bash', [script], {
    env: { ...process.env, BASH_ENV: hook, TMPDIR: scratch, SMOKE_ARGV: record },
    encoding: 'utf8',
    timeout: 15_000,
  });
  assert.equal(result.error, undefined);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /synthetic pin diagnostic/);

  const lines = readFileSync(record, 'utf8').trim().split('\n');
  const by = (cmd) => lines.filter((l) => l.startsWith(`${cmd} `));
  const full = /^(\w+) (\d+) (.+)$/;
  const mk = by('mktemp');
  assert.equal(mk.length, 1);
  const [, , mkArgc, template] = full.exec(mk[0]);
  assert.equal(mkArgc, '1', 'mktemp receives only the template');
  assert.equal(template, `${scratch}/locus-hub-smoke-err.XXXXXX`);
  // head -c N <file>: three args, the file being the complete path.
  const heads = by('head');
  assert.equal(heads.length, 3);
  for (const h of heads) {
    const m = full.exec(h);
    assert.equal(m[2], '3');
    assert.ok(m[3].startsWith(`${scratch}/locus-hub-smoke-err.`));
  }
  const rms = by('rm');
  assert.equal(rms.length, 1);
  const rm = full.exec(rms[0]);
  assert.equal(rm[2], '5', 'rm -rf -- home proj err');
  assert.ok(rm[3].startsWith(`${scratch}/locus-hub-smoke-err.`));
});
