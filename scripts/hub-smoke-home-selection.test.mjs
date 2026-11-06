// Run: node --test scripts/hub-smoke-home-selection.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, normalize } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-smoke.sh');

// Stubs every external command so only SMOKE_HOME selection is exercised.
// The mktemp shim records the SMOKE_HOME template and the directory created;
// a template outside $CONTROLLED is redirected there so the test never writes
// to the real /tmp. The locus stub records whether LOCUS_HOME existed at init.
const hook = `mktemp() {
  local last="\${!#}" args=("$@") path
  if [[ "$last" == */locus-hub-smoke.XXXXXX ]]; then
    printf '%s\\n' "$last" > "$HOME_TEMPLATE"
    [[ "$last" == "$CONTROLLED"/* ]] || args[$(($# - 1))]="$CONTROLLED/locus-hub-smoke.XXXXXX"
    path="$(command mktemp "\${args[@]}")" || return
    printf '%s\\n' "$path" > "$HOME_CREATED"
    printf '%s\\n' "$path"
    return
  fi
  command mktemp "\${args[@]}"
}
jq() { cat >/dev/null; printf '%s\\n' grant; }
cargo() { return 0; }
locus() {
  if [[ "\${1:-}" == init ]]; then
    [[ -d "\${LOCUS_HOME:-}" && "$LOCUS_HOME" == "$(cat "$HOME_CREATED")" ]] || return 73
    : > "$HOME_USED"
  fi
  if [[ "\${1:-} \${2:-}" == 'status --oneline' ]]; then printf '%s\\n' personal:tenant; return 0; fi
  printf '%s\\n' '{}'
}
`;

function run(t, tmpdirEnv) {
  const root = mkdtempSync('/tmp/lhs-home-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const controlled = join(root, 'controlled');
  const files = { template: join(root, 'template'), created: join(root, 'created'), used: join(root, 'used') };
  mkdirSync(controlled);
  writeFileSync(join(root, 'hook.sh'), hook);
  const env = { ...process.env, BASH_ENV: join(root, 'hook.sh'), CONTROLLED: controlled,
    HOME_TEMPLATE: files.template, HOME_CREATED: files.created, HOME_USED: files.used };
  delete env.TMPDIR;
  Object.assign(env, tmpdirEnv(controlled));
  const result = spawnSync('bash', [script], { env, encoding: 'utf8', timeout: 15_000 });
  assert.equal(result.error, undefined);
  assert.equal(result.status, 0, 'smoke must complete successfully');
  assert.doesNotMatch(result.stdout, /^skip\s/m);
  assert.ok(existsSync(files.used), 'LOCUS_HOME must be the created directory during init');
  return {
    controlled,
    template: readFileSync(files.template, 'utf8').trim(),
    created: readFileSync(files.created, 'utf8').trim(),
  };
}

test('SMOKE_HOME is created under the controlled TMPDIR and removed on exit', (t) => {
  const { controlled, template, created } = run(t, (dir) => ({ TMPDIR: dir }));
  assert.equal(template, `${controlled}/locus-hub-smoke.XXXXXX`);
  assert.equal(dirname(created), controlled);
  assert.match(created.slice(controlled.length + 1), /^locus-hub-smoke\.[A-Za-z0-9]{6}$/);
  assert.deepEqual(readdirSync(controlled), []);
});

test('SMOKE_HOME stays directly under a TMPDIR with a trailing slash', (t) => {
  const { controlled, template, created } = run(t, (dir) => {
    writeFileSync(join(dir, 'unrelated'), 'keep');
    return { TMPDIR: `${dir}/` };
  });
  assert.equal(template, `${controlled}//locus-hub-smoke.XXXXXX`);
  assert.equal(dirname(normalize(created)), controlled);
  assert.match(created.slice(created.lastIndexOf('/') + 1), /^locus-hub-smoke\.[A-Za-z0-9]{6}$/);
  assert.equal(existsSync(created), false, 'the allocated home must be removed on exit');
  assert.deepEqual(readdirSync(controlled), ['unrelated']);
  assert.equal(readFileSync(join(controlled, 'unrelated'), 'utf8'), 'keep');
});

for (const [label, tmpdirEnv] of [['unset', () => ({})], ['empty', () => ({ TMPDIR: '' })]]) {
  test(`SMOKE_HOME falls back to /tmp when TMPDIR is ${label}`, (t) => {
    const { controlled, template } = run(t, tmpdirEnv);
    assert.equal(template, '/tmp/locus-hub-smoke.XXXXXX');
    assert.deepEqual(readdirSync(controlled), [], 'redirected directory must be cleaned up');
  });
}
