// Offline checksum-verification tests for scripts/install.sh.
// A stub curl serves a locally built archive and sidecar; nothing touches the
// network or the real HOME. Run: node --test scripts/install-checksum.test.mjs
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import {
  chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { after, before, test } from 'node:test';
import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';

const INSTALLER = resolve(dirname(fileURLToPath(import.meta.url)), 'install.sh');
const VERSION = 'v9.9.9';
const TARGET = 'x86_64-unknown-linux-gnu';
const ASSET = `locus-${TARGET}.tar.gz`;
const OLD = 'old-binary\n';

let root;
let archivePath;
let archiveSha;
let stubDir;

const sha256 = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');

before(() => {
  root = mkdtempSync(join(tmpdir(), 'locus-checksum-test-'));
  const pkg = join(root, 'pkg', `locus-${TARGET}`);
  mkdirSync(pkg, { recursive: true });
  writeFileSync(join(pkg, 'locus'), '#!/bin/sh\necho new-locus\n');
  writeFileSync(join(pkg, 'locus-mcp'), '#!/bin/sh\necho new-mcp\n');
  archivePath = join(root, ASSET);
  const tar = spawnSync('tar', ['-czf', archivePath, '-C', join(root, 'pkg'), `locus-${TARGET}`]);
  assert.equal(tar.status, 0, String(tar.stderr));
  archiveSha = sha256(archivePath);

  // Stub curl/uname are first on PATH; everything else is the real toolchain.
  stubDir = join(root, 'stubs');
  mkdirSync(stubDir);
  writeFileSync(join(stubDir, 'uname'), '#!/bin/sh\ncase "$1" in -s) echo Linux;; -m) echo x86_64;; esac\n');
  writeFileSync(join(stubDir, 'curl'), `#!/bin/bash
out=; url=
while (($#)); do
  case "$1" in
    --output) out=$2; shift 2 ;;
    --retry|--connect-timeout|--max-time|--proto|--proto-redir|--write-out) shift 2 ;;
    https://*) url=$1; shift ;;
    *) shift ;;
  esac
done
case "$url" in
  *.sha256) cp "$STUB_SIDECAR" "$out" ;;
  *.tar.gz) cp "$STUB_ARCHIVE" "$out" ;;
  *) exit 22 ;;
esac
`);
  chmodSync(join(stubDir, 'uname'), 0o755);
  chmodSync(join(stubDir, 'curl'), 0o755);
});

after(() => rmSync(root, { recursive: true, force: true }));

// Runs the installer with a given sidecar body. Returns result plus bin dir.
function install(sidecar, { existing = true } = {}) {
  const dir = mkdtempSync(join(root, 'case-'));
  const binDir = join(dir, 'bin');
  mkdirSync(binDir);
  if (existing) {
    for (const name of ['locus', 'locus-mcp']) writeFileSync(join(binDir, name), OLD);
  }
  const sidecarPath = join(dir, 'sidecar');
  writeFileSync(sidecarPath, sidecar);
  const res = spawnSync('bash', [INSTALLER, '--version', VERSION, '--bin-dir', binDir], {
    encoding: 'utf8',
    env: {
      PATH: `${stubDir}:${process.env.PATH}`,
      HOME: dir,
      TMPDIR: dir,
      STUB_ARCHIVE: archivePath,
      STUB_SIDECAR: sidecarPath,
    },
  });
  return { res, binDir, dir };
}

const content = (binDir, name) => readFileSync(join(binDir, name), 'utf8');
const leftovers = (dir, binDir) => [
  ...readdirSync(dir).filter((n) => n.startsWith('locus-install.')),
  ...readdirSync(binDir).filter((n) => n.startsWith('.locus-install.')),
];

test('matching checksum installs both binaries over existing ones', () => {
  const { res, binDir, dir } = install(`${archiveSha}  ${ASSET}\n`);
  assert.equal(res.status, 0, res.stderr);
  assert.match(res.stdout, /Installed locus and locus-mcp v9\.9\.9/);
  assert.match(content(binDir, 'locus'), /new-locus/);
  assert.match(content(binDir, 'locus-mcp'), /new-mcp/);
  assert.deepEqual(leftovers(dir, binDir), []);
});

test('matching checksum installs into a fresh directory', () => {
  const { res, binDir } = install(`${archiveSha}  ${ASSET}\n`, { existing: false });
  assert.equal(res.status, 0, res.stderr);
  assert.match(content(binDir, 'locus'), /new-locus/);
});

test('uppercase digest and binary-mode marker are accepted', () => {
  const { res, binDir } = install(`${archiveSha.toUpperCase()} *${ASSET}\r\n`);
  assert.equal(res.status, 0, res.stderr);
  assert.match(content(binDir, 'locus'), /new-locus/);
});

test('checksum mismatch fails and leaves existing binaries untouched', () => {
  const wrong = createHash('sha256').update('not the archive').digest('hex');
  const { res, binDir, dir } = install(`${wrong}  ${ASSET}\n`);
  assert.notEqual(res.status, 0);
  assert.match(res.stderr, /SHA-256 checksum mismatch; nothing installed/);
  assert.doesNotMatch(res.stdout, /Installed/);
  assert.equal(content(binDir, 'locus'), OLD);
  assert.equal(content(binDir, 'locus-mcp'), OLD);
  assert.deepEqual(leftovers(dir, binDir), []);
});

test('checksum mismatch installs nothing when no binary existed', () => {
  const wrong = '0'.repeat(64);
  const { res, binDir } = install(`${wrong}  ${ASSET}\n`, { existing: false });
  assert.notEqual(res.status, 0);
  assert.equal(existsSync(join(binDir, 'locus')), false);
  assert.equal(existsSync(join(binDir, 'locus-mcp')), false);
});

for (const [name, body] of [
  ['empty sidecar', ''],
  ['short digest', `${'a'.repeat(63)}  ${ASSET}\n`],
  ['wrong asset name', `${archiveSha}  other.tar.gz\n`],
  ['path in asset name', `${archiveSha}  ../${ASSET}\n`],
  ['extra field', `${archiveSha}  ${ASSET} extra\n`],
  ['two records', `${archiveSha}  ${ASSET}\n${archiveSha}  ${ASSET}\n`],
]) {
  test(`invalid sidecar (${name}) fails without replacing binaries`, () => {
    const { res, binDir } = install(body);
    assert.notEqual(res.status, 0);
    assert.match(res.stderr, /invalid checksum sidecar|exactly one record/);
    assert.equal(content(binDir, 'locus'), OLD);
    assert.equal(content(binDir, 'locus-mcp'), OLD);
  });
}
