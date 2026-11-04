// Run: node --test scripts/e2e-security-home.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const source = readFileSync(join(root, 'scripts', 'e2e.sh'), 'utf8');

// Runs the script's own create_security_home and the section 24 containment
// check verbatim, with log/ok/die stubs and an optional mktemp shim.
function block() {
  const fn = source.match(/^create_security_home\(\) \{\n[\s\S]*?^\}\n/m);
  const begin = source.indexOf('create_security_home\ncreate_security_workspace\n');
  const endMarker = 'ok "security home is created directly beneath TMPDIR"\nfi\n';
  const end = source.indexOf(endMarker, begin);
  assert.ok(fn && begin >= 0 && end > begin, 'e2e.sh must define and check the security home');
  return `${fn[0]}
create_security_workspace() { SECURITY_WS=""; }
${source.slice(begin, end + endMarker.length)}`;
}

const prelude = `set -euo pipefail
ok() { printf 'ok %s\\n' "$*"; }
die() { printf 'FAIL %s\\n' "$*" >&2; exit 1; }
`;

function probe(t, { tmpdir: tmp, shim = '', env = {} }) {
  const dir = mkdtempSync(join(tmpdir(), 'locus-sec-home-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const full = { ...process.env, OBSERVED: join(dir, 'observed'), ...env };
  if (tmp === undefined) delete full.TMPDIR; else full.TMPDIR = tmp(dir);
  const result = spawnSync('bash', ['-c', `${prelude}${shim}
${block()}
printf '%s' "$SECURITY_HOME" > "$OBSERVED"
[[ -d "$SECURITY_HOME" ]] && rm -rf "$SECURITY_HOME"
`], { env: full, encoding: 'utf8' });
  let home = null;
  try { home = readFileSync(join(dir, 'observed'), 'utf8'); } catch {}
  return { dir, result, home };
}

for (const [label, name] of [['a plain', 'tmp'], ['a spaced', 'tmp with space']]) {
  test(`security home is created directly beneath ${label} controlled TMPDIR`, (t) => {
    const { dir, result, home } = probe(t, {
      tmpdir: (d) => { const p = join(d, name); mkdirSync(p); return p; },
    });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(dirname(home), join(dir, name));
    assert.match(home, /\/locus-security-e2e\.[^/]+$/);
    assert.match(result.stdout, /ok security home is created directly beneath TMPDIR/);
    assert.deepEqual(readdirSync(join(dir, name)), [], 'nothing leaks outside the removed home');
  });
}

for (const [label, tmp] of [['unset', undefined], ['empty', () => '']]) {
  test(`security home falls back to /tmp when TMPDIR is ${label}`, (t) => {
    const { result, home } = probe(t, { tmpdir: tmp });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(dirname(home), '/tmp');
    assert.match(home, /\/locus-security-e2e\.[^/]+$/);
    assert.doesNotMatch(result.stdout, /beneath TMPDIR/, 'no containment check without a supplied TMPDIR');
  });
}

test('a security home created outside TMPDIR fails the containment check', (t) => {
  const { result, home } = probe(t, {
    tmpdir: (d) => { mkdirSync(join(d, 'tmp')); mkdirSync(join(d, 'wrong')); return join(d, 'tmp'); },
    shim: `mktemp() { command mktemp -d "$(dirname "$TMPDIR")/wrong/locus-security-e2e.XXXXXX"; }\n`,
  });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /FAIL SECURITY_HOME was not created directly beneath TMPDIR/);
  assert.equal(home, null, 'the probe must stop before observing the home');
});
