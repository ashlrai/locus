// Run: node --test scripts/install-destination.test.mjs
// Destination-path regression tests for scripts/install.sh. A stubbed curl
// serves a local release fixture, so nothing is downloaded and the real HOME
// is never touched. Every destination below contains spaces (or other shell
// metacharacters) and must be treated as exactly one path.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import {
  chmodSync, existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, readdirSync,
  rmSync, symlinkSync, writeFileSync,
} from 'node:fs';
import { createHash } from 'node:crypto';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const bash = spawnSync('bash', ['-c', 'command -v bash'], { encoding: 'utf8' }).stdout.trim();
const installer = join(dirname(fileURLToPath(import.meta.url)), 'install.sh');
const target = 'x86_64-unknown-linux-gnu';
const version = 'v1.2.3';
const LOCUS = '#!/bin/sh\nprintf "locus fixture\\n"\n';
const MCP = '#!/bin/sh\nprintf "mcp fixture\\n"\n';

function which(tool) {
  const r = spawnSync('bash', ['-c', `command -v ${tool}`], { encoding: 'utf8' });
  return r.status === 0 ? r.stdout.trim() : null;
}

// Builds an isolated case: restricted PATH (mock curl/uname + real basics),
// a release archive with matching checksum, and a spaced working area.
function setup(t) {
  const base = mkdtempSync(join(tmpdir(), 'locus install dest-'));
  t.after(() => rmSync(base, { recursive: true, force: true }));
  const bin = join(base, 'tools');
  const stage = join(base, 'stage', `locus-${target}`);
  const home = join(base, 'home dir');
  const tmp = join(base, 'tmp dir');
  for (const d of [bin, stage, home, tmp]) mkdirSync(d, { recursive: true });

  for (const tool of ['tar', 'gzip', 'mkdir', 'mktemp', 'chmod', 'mv', 'rm', 'tr', 'cat', 'cp']) {
    const p = which(tool);
    if (p) symlinkSync(p, join(bin, tool));
  }
  const sum = which('sha256sum');
  const shasum = which('shasum');
  assert.ok(sum || shasum, 'need sha256sum or shasum');
  symlinkSync(sum ?? shasum, join(bin, sum ? 'sha256sum' : 'shasum'));
  const stub = (name, body) => {
    writeFileSync(join(bin, name), `#!/bin/bash\n${body}\n`);
    chmodSync(join(bin, name), 0o755);
  };
  stub('uname', 'case "$1" in -s) echo Linux;; -m) echo x86_64;; *) exit 2;; esac');
  stub('curl', `out=; url=
while (($#)); do case "$1" in
  --output) out=$2; shift 2;;
  --write-out|--proto|--proto-redir|--retry|--connect-timeout|--max-time) shift 2;;
  https://*) url=$1; shift;;
  *) shift;; esac; done
case "$url" in
  *.sha256) cp "${base.replaceAll('"', '\\"')}/release.sha256" "$out";;
  *.tar.gz) cp "${base.replaceAll('"', '\\"')}/release.tar.gz" "$out";;
  *) exit 22;; esac`);

  writeFileSync(join(stage, 'locus'), LOCUS);
  writeFileSync(join(stage, 'locus-mcp'), MCP);
  const archive = join(base, 'release.tar.gz');
  const r = spawnSync('tar', ['-czf', archive, '-C', join(base, 'stage'), `locus-${target}`]);
  assert.equal(r.status, 0);
  const digest = createHash('sha256').update(readFileSync(archive)).digest('hex');
  writeFileSync(join(base, 'release.sha256'), `${digest}  locus-${target}.tar.gz\n`);

  return { base, bin, home, tmp };
}

function install(c, args, { cwd = c.base } = {}) {
  return spawnSync(bash, [installer, '--version', version, ...args], {
    cwd, encoding: 'utf8',
    env: { HOME: c.home, TMPDIR: c.tmp, PATH: c.bin },
  });
}

function assertInstalled(dest) {
  for (const [name, body] of [['locus', LOCUS], ['locus-mcp', MCP]]) {
    const p = join(dest, name);
    assert.equal(readFileSync(p, 'utf8'), body, `${name} bytes`);
    assert.ok((lstatSync(p).mode & 0o111) !== 0, `${name} executable`);
  }
  assert.deepEqual(readdirSync(dest).sort(), ['locus', 'locus-mcp'], 'no staging leftovers');
}

test('absolute --bin-dir containing spaces', (t) => {
  const c = setup(t);
  const dest = join(c.base, 'my install dir', 'bin');
  const r = install(c, ['--bin-dir', dest]);
  assert.equal(r.status, 0, r.stderr);
  assertInstalled(dest);
  assert.ok(r.stdout.includes(`in ${dest}\n`), 'reports the full spaced path');
  assert.ok(r.stdout.includes(`PATH: ${dest}\n`), 'PATH hint keeps the full path');
  assert.deepEqual(readdirSync(c.tmp), [], 'no temp files leaked');
});

test('relative --bin-dir with spaces resolves against a spaced cwd', (t) => {
  const c = setup(t);
  const cwd = join(c.base, 'work space');
  mkdirSync(cwd);
  const r = install(c, ['--bin-dir', 'rel dir/bin'], { cwd });
  assert.equal(r.status, 0, r.stderr);
  assertInstalled(join(cwd, 'rel dir', 'bin'));
});

test('default destination under a HOME containing spaces', (t) => {
  const c = setup(t);
  const r = install(c, []);
  assert.equal(r.status, 0, r.stderr);
  assertInstalled(join(c.home, '.local', 'bin'));
});

test('destination with leading/trailing/multiple spaces and shell metacharacters', (t) => {
  const c = setup(t);
  for (const name of [' lead', 'trail ', 'two  spaces', 'a$HOME b', 'q"uo\'te s', 'glob * ? [x]', '-dash dir']) {
    const dest = join(c.base, 'meta', name);
    const r = install(c, ['--bin-dir', dest]);
    assert.equal(r.status, 0, `${name}: ${r.stderr}`);
    assertInstalled(dest);
  }
  assert.ok(!existsSync(join(c.base, 'meta', 'a')), 'no word-split sibling created');
});

test('upgrade replaces existing binaries in a spaced destination', (t) => {
  const c = setup(t);
  const dest = join(c.base, 'old bin');
  mkdirSync(dest);
  writeFileSync(join(dest, 'locus'), 'old locus\n');
  writeFileSync(join(dest, 'locus-mcp'), 'old mcp\n');
  const r = install(c, ['--bin-dir', dest]);
  assert.equal(r.status, 0, r.stderr);
  assertInstalled(dest);
});

test('spaced destination already on PATH omits the PATH hint', (t) => {
  const c = setup(t);
  const dest = join(c.base, 'on path');
  const r = spawnSync(bash, [installer, '--version', version, '--bin-dir', dest], {
    cwd: c.base, encoding: 'utf8',
    env: { HOME: c.home, TMPDIR: c.tmp, PATH: `${c.bin}:${dest}` },
  });
  assert.equal(r.status, 0, r.stderr);
  assert.ok(!r.stdout.includes('Add this directory'), r.stdout);
});

test('spaced destination with a symlinked binary is refused, nothing written', (t) => {
  const c = setup(t);
  const dest = join(c.base, 'sym dir');
  mkdirSync(dest);
  const victim = join(c.base, 'victim file');
  writeFileSync(victim, 'keep\n');
  symlinkSync(victim, join(dest, 'locus'));
  const r = install(c, ['--bin-dir', dest]);
  assert.notEqual(r.status, 0);
  assert.match(r.stderr, /refusing symlink or directory destination/);
  assert.equal(readFileSync(victim, 'utf8'), 'keep\n');
  assert.deepEqual(readdirSync(c.tmp), [], 'no temp files leaked');
});

test('uncreatable spaced destination fails cleanly', (t) => {
  const c = setup(t);
  const file = join(c.base, 'a file');
  writeFileSync(file, 'x');
  const r = install(c, ['--bin-dir', join(file, 'sub dir')]);
  assert.notEqual(r.status, 0);
  assert.match(r.stderr, /cannot create install directory/);
  assert.deepEqual(readdirSync(c.tmp), [], 'no temp files leaked');
});
