// Run: node --test scripts/hub-smoke-spaced-home.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-smoke.sh');

test('SMOKE_HOME is created at the intended path under a space-containing TMPDIR', (t) => {
  const root = mkdtempSync('/tmp/lhs-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'my temp dir');
  const record = join(root, 'home-seen');
  const hook = join(root, 'hook.sh');
  mkdirSync(scratch);
  writeFileSync(hook, `cargo() { return 0; }
jq() { cat >/dev/null; printf '%s\\n' grant; }
locus() {
  case "\${1:-}" in
    init)
      [[ -d "$LOCUS_HOME" && "$(dirname "$LOCUS_HOME")" == "$TMPDIR" \\
        && "$(basename "$LOCUS_HOME")" == locus-hub-smoke.?????? ]] || return 73
      printf '%s' "$LOCUS_HOME" > "$HOME_RECORD" ;;
    status) [[ "\${2:-}" == --oneline ]] && { printf '%s\\n' personal:tenant; return; } ;;
  esac
  printf '%s\\n' '{}'
}
`);
  const result = spawnSync('bash', [script], {
    env: { ...process.env, BASH_ENV: hook, TMPDIR: scratch, HOME_RECORD: record },
    encoding: 'utf8',
  });
  assert.equal(result.error, undefined);
  assert.equal(result.status, 0, result.stderr);
  assert.ok(existsSync(record), 'init must have seen an existing LOCUS_HOME');
  const home = readFileSync(record, 'utf8');
  assert.equal(dirname(home), scratch);
  assert.ok(result.stdout.includes(`== hub-smoke LOCUS_HOME=${home} ==`));
  assert.equal(existsSync(home), false, 'SMOKE_HOME must be removed on exit');
});

test('SMOKE_HOME is private (0700) when created under a spaced TMPDIR', (t) => {
  const root = mkdtempSync('/tmp/lhs-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'a b');
  const record = join(root, 'mode');
  const hook = join(root, 'hook.sh');
  mkdirSync(scratch);
  writeFileSync(hook, `cargo() { return 0; }
jq() { cat >/dev/null; printf '%s\\n' grant; }
locus() {
  if [[ "\${1:-}" == init ]]; then
    stat -f %Lp "$LOCUS_HOME" 2>/dev/null > "$MODE_RECORD" || stat -c %a "$LOCUS_HOME" > "$MODE_RECORD"
  fi
  if [[ "\${1:-}" == status && "\${2:-}" == --oneline ]]; then printf '%s\\n' personal:tenant; else printf '%s\\n' '{}'; fi
}
`);
  const result = spawnSync('bash', [script], {
    env: { ...process.env, BASH_ENV: hook, TMPDIR: scratch, MODE_RECORD: record },
    encoding: 'utf8',
  });
  assert.equal(result.status, 0, result.stderr);
  assert.equal(readFileSync(record, 'utf8').trim(), '700');
  assert.ok(statSync(scratch).isDirectory());
});
