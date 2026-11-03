// Run: node --test scripts/hub-smoke-err-selection.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-smoke.sh');

// Stubs every external command so only the temp-file selection is exercised.
// The mktemp shim records the SMOKE_ERR template; when the template is not
// under $CONTROLLED it is redirected there, so the test never writes to the
// real /tmp.
const hook = `mktemp() {
  local last="\${!#}" args=("$@")
  if [[ "$last" == */locus-hub-smoke-err.XXXXXX ]]; then
    printf '%s\\n' "$last" > "$ERR_TEMPLATE"
    [[ "$last" == "$CONTROLLED"/* ]] || args[$(($# - 1))]="$CONTROLLED/locus-hub-smoke-err.XXXXXX"
  fi
  command mktemp "\${args[@]}"
}
jq() { cat >/dev/null; printf '%s\\n' grant; }
cargo() { return 0; }
locus() {
  if [[ "\${1:-} \${2:-}" == 'status --oneline' ]]; then printf '%s\\n' personal:tenant; return 0; fi
  printf '%s\\n' '{}'
}
`;

function run(t, name, tmpdirEnv) {
  const root = mkdtempSync('/tmp/lhs-sel-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const controlled = join(root, name);
  const template = join(root, 'template');
  mkdirSync(controlled);
  writeFileSync(join(root, 'hook.sh'), hook);
  const env = { ...process.env, BASH_ENV: join(root, 'hook.sh'),
    CONTROLLED: controlled, ERR_TEMPLATE: template };
  delete env.TMPDIR;
  Object.assign(env, tmpdirEnv(controlled));
  const result = spawnSync('bash', [script], { env, encoding: 'utf8', timeout: 15_000 });
  assert.equal(result.error, undefined);
  assert.equal(result.status, 0, 'smoke must complete successfully');
  assert.doesNotMatch(result.stdout, /^skip\s/m);
  return { controlled, template: readFileSync(template, 'utf8').trim() };
}

test('SMOKE_ERR is created under the controlled TMPDIR and removed on exit', (t) => {
  const { controlled, template } = run(t, 'controlled', (dir) => ({ TMPDIR: dir }));
  assert.equal(template, `${controlled}/locus-hub-smoke-err.XXXXXX`);
  assert.deepEqual(readdirSync(controlled), []);
});

for (const [label, tmpdirEnv] of [['unset', () => ({})], ['empty', () => ({ TMPDIR: '' })]]) {
  test(`SMOKE_ERR falls back to /tmp when TMPDIR is ${label}`, (t) => {
    const { controlled, template } = run(t, 'fallback', tmpdirEnv);
    assert.equal(template, '/tmp/locus-hub-smoke-err.XXXXXX');
    assert.deepEqual(readdirSync(controlled), [], 'redirected file must be cleaned up');
  });
}
