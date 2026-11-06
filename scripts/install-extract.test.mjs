import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, readdirSync, rmSync, chmodSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

const installer = fileURLToPath(new URL('./install.sh', import.meta.url));
const target = 'x86_64-unknown-linux-gnu';

function command(cmd, args, options = {}) {
  const result = spawnSync(cmd, args, { encoding: 'utf8', ...options });
  assert.equal(result.status, 0, `${cmd} failed: ${result.stderr}`);
}

for (const failure of ['corrupt archive', 'locus extraction', 'locus-mcp extraction']) {
  test(`cleans temporary files after ${failure}`, (t) => {
    const root = mkdtempSync(join(tmpdir(), 'locus-extract-test-'));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const home = join(root, 'home');
    const temp = join(root, 'tmp');
    const tools = join(root, 'tools');
    const dest = join(root, 'destination');
    const fixture = join(root, `locus-${target}`);
    for (const dir of [home, temp, tools, dest, fixture]) mkdirSync(dir);
    for (const binary of ['locus', 'locus-mcp']) {
      writeFileSync(join(fixture, binary), `new ${binary}\n`);
      writeFileSync(join(dest, binary), `old ${binary}\n`);
    }
    const archive = join(root, 'archive.tar.gz');
    command('tar', ['-czf', archive, '-C', root, `locus-${target}`]);
    if (failure === 'corrupt archive') writeFileSync(archive, 'not a gzip archive\n');
    const digest = createHash('sha256').update(readFileSync(archive)).digest('hex');
    const sidecar = join(root, 'checksum');
    writeFileSync(sidecar, `${digest}  locus-${target}.tar.gz\n`);
    const mock = (name, contents) => {
      const path = join(tools, name);
      writeFileSync(path, `#!/bin/bash\nset -eu\n${contents}\n`);
      chmodSync(path, 0o755);
    };
    mock('uname', 'case "$1" in -s) echo Linux;; -m) echo x86_64;; esac');
    mock('curl', 'while [ "$#" -gt 0 ]; do case "$1" in --output) out=$2; shift 2;; https://*) url=$1; shift;; *) shift;; esac; done\ncase "$url" in *.sha256) cp "$FIXTURE_CHECKSUM" "$out";; *) cp "$FIXTURE_ARCHIVE" "$out";; esac');
    if (failure !== 'corrupt archive') {
      const failedBinary = failure.split(' ')[0];
      mock('tar', `if [[ "$1" == -xOzf && "$3" == */${failedBinary} ]]; then printf partial; exit 1; fi\nexec "$REAL_TAR" "$@"`);
    }
    const realTar = spawnSync('which', ['tar'], { encoding: 'utf8' }).stdout.trim();
    const result = spawnSync('bash', [installer, '--version', 'v1.2.3', '--bin-dir', dest], {
      encoding: 'utf8',
      env: { ...process.env, HOME: home, TMPDIR: temp, PATH: `${tools}:${process.env.PATH}`, FIXTURE_ARCHIVE: archive, FIXTURE_CHECKSUM: sidecar, REAL_TAR: realTar },
    });
    assert.equal(result.signal, null);
    assert.notEqual(result.status, null);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, failure === 'corrupt archive' ? /archive must contain locus and locus-mcp/ : new RegExp(`could not unpack ${failure.split(' ')[0]}`));
    assert.deepEqual(readdirSync(temp), []);
    assert.deepEqual(readdirSync(dest).sort(), ['locus', 'locus-mcp']);
    for (const binary of ['locus', 'locus-mcp']) assert.equal(readFileSync(join(dest, binary), 'utf8'), `old ${binary}\n`);
  });
}
