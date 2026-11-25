// Run: node --test scripts/hub-smoke-unset-tmpdir.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { basename, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-smoke.sh');

test('unset TMPDIR creates all smoke resources in /tmp and cleans them on exit offline', (t) => {
  const root = mkdtempSync('/tmp/lhs-unset-');
  const record = join(root, 'resources');
  t.after(() => {
    // Also remove observed allocations if an assertion or the script fails.
    if (existsSync(record)) {
      for (const path of readFileSync(record, 'utf8').trim().split('\n')) {
        if (dirname(path) === '/tmp' && /^locus-hub-(smoke|proj|smoke-err)\.[A-Za-z0-9]+$/.test(basename(path))) {
          rmSync(path, { recursive: true, force: true });
        }
      }
    }
    rmSync(root, { recursive: true, force: true });
  });
  const home = join(root, 'home');
  const hook = join(root, 'hook.sh');
  mkdirSync(home);
  // Functions override even installed/built binaries. Only local filesystem
  // operations run: no real credentials, broker, Cargo build, or network.
  writeFileSync(hook, `[[ "\${TMPDIR+x}" != x ]] || exit 90
cargo() { return 0; }
jq() { cat >/dev/null; printf '%s\\n' grant; }
locus() {
  if [[ "\${1:-}" == init ]]; then
    printf '%s\\n' "$SMOKE_HOME" "$SMOKE_PROJ" "$SMOKE_ERR" > "$SMOKE_RECORD"
    [[ -d "$SMOKE_HOME" && -d "$SMOKE_PROJ" && -f "$SMOKE_ERR" ]] || return 91
    mkdir "$SMOKE_HOME/nested" "$SMOKE_PROJ/nested" || return
    touch "$SMOKE_HOME/nested/content" "$SMOKE_PROJ/nested/content" || return
    printf 'synthetic diagnostic\\n' >&2
    : > "$SMOKE_MARKER"
  fi
  if [[ "\${1:-} \${2:-}" == 'status --oneline' ]]; then
    [[ -f .mcp.json ]] || return 92
    printf '%s\\n' personal:tenant
  else
    printf '%s\\n' '{}'
  fi
}
`);
  const marker = join(root, 'observed');
  const env = { ...process.env, HOME: home, BASH_ENV: hook,
    SMOKE_RECORD: record, SMOKE_MARKER: marker,
    LOCUS_CONTROL_CAPABILITY: 'synthetic-test-capability' };
  delete env.TMPDIR;
  const result = spawnSync('bash', [script], { env, encoding: 'utf8', timeout: 15_000 });

  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  assert.equal(result.status, 0);
  assert.match(result.stdout, /^== hub-smoke OK ==$/m);
  assert.doesNotMatch(result.stdout, /^(skip|FAIL)\s/m);
  assert.ok(existsSync(marker), 'all three resources must exist during initialization');
  const paths = readFileSync(record, 'utf8').trim().split('\n');
  assert.equal(paths.length, 3);
  assert.equal(new Set(paths).size, 3);
  for (const [index, prefix] of ['locus-hub-smoke.', 'locus-hub-proj.', 'locus-hub-smoke-err.'].entries()) {
    assert.equal(dirname(paths[index]), '/tmp');
    assert.ok(basename(paths[index]).startsWith(prefix));
    assert.match(basename(paths[index]).slice(prefix.length), /^[A-Za-z0-9]{6}$/);
    assert.equal(existsSync(paths[index]), false, `${prefix} must be removed before test teardown`);
  }
});
