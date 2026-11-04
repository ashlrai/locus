// Run: node --test scripts/hub-smoke-proj-spaced-tmpdir.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-smoke.sh');

test('SMOKE_PROJ with a space-containing TMPDIR is created at the intended path, navigated into, and cleaned up', (t) => {
  const root = mkdtempSync('/tmp/lhs-proj-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scratch = join(root, 'project workspace dir');
  const record = join(root, 'argv');
  const hook = join(root, 'hook.sh');
  mkdirSync(scratch);
  // Intercept mktemp/cd/rm to record argv and verify the spaced path is never word-split.
  // init/pin succeed so the script reaches the project-creation and cd steps.
  // jq/grep always pass so every check_json() call succeeds.
  writeFileSync(hook, `mktemp() {
  local last="\${!#}"
  [[ "\$last" == "\$TMPDIR"/locus-hub-proj.XXXXXX ]] && printf 'mktemp %s %s\\n' "\$#" "\$last" >> "\$SMOKE_ARGV"
  command mktemp "\$@"
}
cd() {
  local target="\${1:-}"
  printf 'cd %s %s\\n' "\$#" "\$target" >> "\$SMOKE_ARGV"
  command cd "\$@"
}
rm() {
  local arg
  for arg in "\$@"; do
    [[ "\$arg" == "\$TMPDIR"/* ]] && printf 'rm %s %s\\n' "\$#" "\$arg" >> "\$SMOKE_ARGV"
  done
  command rm "\$@"
}
grep() { return 1; }
cargo() { return 0; }
sleep() { :; }
jq() { cat >/dev/null; printf '%s\\n' ok; }
locus() {
  case "\${1:-}" in
    init) return 0 ;;
    pin) return 0 ;;
    leave) return 0 ;;
    status)
      if [[ "\${2:-}" == --oneline ]]; then
        printf '%s\\n' personal:tenant
      fi
      return 0
      ;;
  esac
  printf '%s\\n' '{}'
}
`);
  const result = spawnSync('bash', [script], {
    env: { ...process.env, BASH_ENV: hook, TMPDIR: scratch, SMOKE_ARGV: record },
    encoding: 'utf8',
    timeout: 15_000,
  });

  assert.equal(result.error, undefined, 'spawn must not error');
  assert.equal(result.signal, null, 'must not be killed by signal');
  assert.equal(result.status, 0, 'smoke must complete successfully');
  assert.match(result.stdout, /== hub-smoke OK ==/, 'success banner missing');

  const lines = readFileSync(record, 'utf8').trim().split('\n');
  const by = (cmd) => lines.filter((l) => l.startsWith(`${cmd} `));

  // mktemp received the template as a single argument (2 args: -d and full template)
  const mk = by('mktemp');
  assert.equal(mk.length, 1, 'mktemp called once for SMOKE_PROJ');
  const mkParts = mk[0].split(' ');
  assert.equal(mkParts[1], '2', 'mktemp receives -d and the full template as two arguments');
  assert.equal(mkParts.slice(2).join(' '), `${scratch}/locus-hub-proj.XXXXXX`);

  // cd navigated into SMOKE_PROJ under the spaced TMPDIR (filter out subshell cd to ROOT)
  const projCd = by('cd').find((l) => l.includes('locus-hub-proj.'));
  assert.ok(projCd, 'cd must target the SMOKE_PROJ directory');
  const cdParts = projCd.split(' ');
  assert.equal(cdParts[1], '1', 'cd receives one argument');
  assert.ok(cdParts.slice(2).join(' ').startsWith(`${scratch}/locus-hub-proj.`));

  // rm cleanup included the SMOKE_PROJ directory under the spaced TMPDIR
  const rms = by('rm');
  const projRm = rms.find((l) => l.includes('locus-hub-proj.'));
  assert.ok(projRm, 'rm cleanup must include the SMOKE_PROJ directory');
  const rmParts = projRm.split(' ');
  assert.equal(rmParts[1], '5', 'rm -rf -- home proj err: five arguments total');
  assert.ok(rmParts.slice(2).join(' ').startsWith(`${scratch}/locus-hub-proj.`));
});
