// Offline download-failure tests for scripts/install.sh.
// A stub curl fails selected requests; an existing installation must survive.
// Run: node --test scripts/install-download.test.mjs
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import {
  chmodSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync,
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
let sidecarPath;
let stubDir;

before(() => {
  root = mkdtempSync(join(tmpdir(), 'locus-download-test-'));
  const pkg = join(root, 'pkg', `locus-${TARGET}`);
  mkdirSync(pkg, { recursive: true });
  writeFileSync(join(pkg, 'locus'), '#!/bin/sh\necho new-locus\n');
  writeFileSync(join(pkg, 'locus-mcp'), '#!/bin/sh\necho new-mcp\n');
  archivePath = join(root, ASSET);
  const tar = spawnSync('tar', ['-czf', archivePath, '-C', join(root, 'pkg'), `locus-${TARGET}`]);
  assert.equal(tar.status, 0, String(tar.stderr));
  const sha = createHash('sha256').update(readFileSync(archivePath)).digest('hex');
  sidecarPath = join(root, 'sidecar');
  writeFileSync(sidecarPath, `${sha}  ${ASSET}\n`);

  // STUB_FAIL selects what breaks: latest | archive | sidecar | partial | all.
  // STUB_CODE is curl's exit status (22 = HTTP error, 6 = DNS, 28 = timeout).
  stubDir = join(root, 'stubs');
  mkdirSync(stubDir);
  writeFileSync(join(stubDir, 'uname'), '#!/bin/sh\ncase "$1" in -s) echo Linux;; -m) echo x86_64;; esac\n');
  writeFileSync(join(stubDir, 'curl'), `#!/bin/bash
out=; url=; wo=
while (($#)); do
  case "$1" in
    --output) out=$2; shift 2 ;;
    --write-out) wo=$2; shift 2 ;;
    --retry|--connect-timeout|--max-time|--proto|--proto-redir) shift 2 ;;
    https://*) url=$1; shift ;;
    *) shift ;;
  esac
done
fail() { exit "\${STUB_CODE:-22}"; }
[[ \${STUB_FAIL:-} == all ]] && fail
case "$url" in
  */releases/latest)
    [[ \${STUB_FAIL:-} == latest ]] && fail
    printf '%s' "https://github.com/ashlrai/locus/releases/tag/${VERSION}" ;;
  *.sha256)
    [[ \${STUB_FAIL:-} == sidecar ]] && fail
    cp "$STUB_SIDECAR" "$out" ;;
  *.tar.gz)
    [[ \${STUB_FAIL:-} == archive ]] && fail
    if [[ \${STUB_FAIL:-} == partial ]]; then head -c 20 "$STUB_ARCHIVE" > "$out"; fail; fi
    cp "$STUB_ARCHIVE" "$out" ;;
  *) exit 22 ;;
esac
`);
  chmodSync(join(stubDir, 'uname'), 0o755);
  chmodSync(join(stubDir, 'curl'), 0o755);
});

after(() => rmSync(root, { recursive: true, force: true }));

function install(fail, { code = 22, args = ['--version', VERSION], existing = true } = {}) {
  const dir = mkdtempSync(join(root, 'case-'));
  const binDir = join(dir, 'bin');
  mkdirSync(binDir);
  if (existing) {
    for (const name of ['locus', 'locus-mcp']) writeFileSync(join(binDir, name), OLD);
  }
  const res = spawnSync('bash', [INSTALLER, ...args, '--bin-dir', binDir], {
    encoding: 'utf8',
    env: {
      PATH: `${stubDir}:${process.env.PATH}`,
      HOME: dir,
      TMPDIR: dir,
      STUB_ARCHIVE: archivePath,
      STUB_SIDECAR: sidecarPath,
      STUB_FAIL: fail,
      STUB_CODE: String(code),
    },
  });
  return { res, binDir, dir };
}

function assertIntact({ res, binDir, dir }, message) {
  assert.notEqual(res.status, 0, 'installer must exit nonzero');
  assert.match(res.stderr, message);
  assert.doesNotMatch(res.stdout, /Installed/);
  assert.deepEqual(readdirSync(binDir).sort(), ['locus', 'locus-mcp']);
  for (const name of ['locus', 'locus-mcp']) {
    assert.equal(readFileSync(join(binDir, name), 'utf8'), OLD);
  }
  assert.deepEqual(readdirSync(dir).filter((n) => n.startsWith('locus-install.')), []);
}

test('control: a working download installs over the existing files', () => {
  const { res, binDir } = install('none');
  assert.equal(res.status, 0, res.stderr);
  assert.match(readFileSync(join(binDir, 'locus'), 'utf8'), /new-locus/);
});

test('archive download failure exits nonzero and keeps the existing install', () => {
  assertIntact(install('archive'), /archive download failed; nothing installed/);
});

test('checksum download failure exits nonzero and keeps the existing install', () => {
  assertIntact(install('sidecar'), /checksum download failed; nothing installed/);
});

test('truncated archive download exits nonzero and keeps the existing install', () => {
  assertIntact(install('partial'), /archive download failed; nothing installed/);
});

test('latest-release lookup failure exits nonzero and keeps the existing install', () => {
  assertIntact(install('latest', { args: [] }), /could not resolve the latest release/);
});

for (const code of [6, 7, 28, 35]) {
  test(`curl exit ${code} (network error) is a failed install, not a success`, () => {
    assertIntact(install('all', { code, args: [] }), /could not resolve the latest release/);
    assertIntact(install('archive', { code }), /archive download failed/);
  });
}

test('failed download into a fresh directory leaves no binaries behind', () => {
  const { res, binDir, dir } = install('archive', { existing: false });
  assert.notEqual(res.status, 0);
  assert.deepEqual(readdirSync(binDir), []);
  assert.deepEqual(readdirSync(dir).filter((n) => n.startsWith('locus-install.')), []);
});
