// Run: node --test scripts/hub-smoke-proj-selection.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-smoke.sh');

// Stubs every external command so only SMOKE_PROJ selection is exercised.
// The mktemp shim records the SMOKE_PROJ template and the directory created;
// a template outside $CONTROLLED is redirected there so the test never writes
// to the real /tmp. The locus stub checks $SMOKE_PROJ at init and records the
// working directory once the script cd's into the project — the script
// writes .mcp.json there before running any checks.
const hook = `mktemp() {
  local last="\${!#}" args=("$@") path
  if [[ "$last" == */locus-hub-proj.XXXXXX ]]; then
    printf '%s\\n' "$last" > "$PROJ_TEMPLATE"
    [[ "$last" == "$CONTROLLED"/* ]] || args[$(($# - 1))]="$CONTROLLED/locus-hub-proj.XXXXXX"
    path="$(command mktemp "\${args[@]}")" || return
    printf '%s\\n' "$path" > "$PROJ_CREATED"
    printf '%s\\n' "$path"
    return
  fi
  command mktemp "\${args[@]}"
}
jq() { cat >/dev/null; printf '%s\\n' grant; }
cargo() { return 0; }
locus() {
  if [[ "\${1:-}" == init ]]; then
    [[ -d "$SMOKE_PROJ" && "$SMOKE_PROJ" == "$(cat "$PROJ_CREATED")" ]] || return 72
  fi
  if [[ ! -e "$PROJ_USED" && "\${PWD##*/}" == locus-hub-proj.* && -f .mcp.json ]]; then
    [[ . -ef "$(cat "$PROJ_CREATED")" ]] || return 73
    printf '%s\\n' "$PWD" > "$PROJ_USED"
  fi
  if [[ "\${1:-} \${2:-}" == 'status --oneline' ]]; then printf '%s\\n' personal:tenant; return 0; fi
  printf '%s\\n' '{}'
}
`;

function run(t, tmpdirEnv) {
  const root = mkdtempSync('/tmp/lhs-proj-sel-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const controlled = join(root, 'controlled');
  const files = { template: join(root, 'template'), created: join(root, 'created'), used: join(root, 'used') };
  mkdirSync(controlled);
  writeFileSync(join(root, 'hook.sh'), hook);
  const env = { ...process.env, BASH_ENV: join(root, 'hook.sh'), CONTROLLED: controlled,
    PROJ_TEMPLATE: files.template, PROJ_CREATED: files.created, PROJ_USED: files.used };
  delete env.TMPDIR;
  Object.assign(env, tmpdirEnv(controlled));
  const result = spawnSync('bash', [script], { env, encoding: 'utf8', timeout: 15_000 });
  assert.equal(result.error, undefined);
  assert.equal(result.status, 0, `stderr:\n${result.stderr}`);
  assert.doesNotMatch(result.stdout, /^skip\s/m);
  assert.ok(existsSync(files.used), 'checks must run with SMOKE_PROJ as the working directory');
  return {
    controlled,
    template: readFileSync(files.template, 'utf8').trim(),
    created: readFileSync(files.created, 'utf8').trim(),
    used: readFileSync(files.used, 'utf8').trim(),
  };
}

test('SMOKE_PROJ is created under the controlled TMPDIR, used as the working directory, and removed on exit', (t) => {
  const { controlled, template, created, used } = run(t, (dir) => ({ TMPDIR: dir }));
  assert.equal(template, `${controlled}/locus-hub-proj.XXXXXX`);
  assert.equal(dirname(created), controlled);
  assert.match(created.slice(controlled.length + 1), /^locus-hub-proj\.[A-Za-z0-9]{6}$/);
  assert.equal(used, created, 'checks run inside the directory mktemp created');
  assert.deepEqual(readdirSync(controlled), []);
});

for (const [label, tmpdirEnv] of [['unset', () => ({})], ['empty', () => ({ TMPDIR: '' })]]) {
  test(`SMOKE_PROJ falls back to /tmp when TMPDIR is ${label}`, (t) => {
    const { controlled, template, used } = run(t, tmpdirEnv);
    assert.equal(template, '/tmp/locus-hub-proj.XXXXXX');
    assert.equal(dirname(used), controlled, 'redirected project is still the working directory');
    assert.deepEqual(readdirSync(controlled), [], 'redirected directory must be cleaned up');
  });
}

test('SMOKE_PROJ stays directly under a TMPDIR given with a trailing slash', (t) => {
  const { controlled, template, created, used } = run(t, (dir) => ({ TMPDIR: `${dir}/` }));
  assert.equal(template, `${controlled}//locus-hub-proj.XXXXXX`);
  // mktemp echoes the template's double slash; bash collapses it in $PWD.
  assert.equal(dirname(created.replace('//', '/')), controlled);
  assert.equal(dirname(used), controlled, 'checks run inside the project beneath the TMPDIR');
  assert.deepEqual(readdirSync(controlled), []);
});
