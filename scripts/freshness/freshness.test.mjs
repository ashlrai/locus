import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, cp, readFile, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { repository, run, sources, replaceBlocks } from './engine.mjs';

const config = JSON.parse(await readFile(join(repository, 'scripts/freshness/blocks.json'), 'utf8'));
const values = { version: '1.2.3', cli: '1.2.3', mcp: '1.2.3', date: '2026-01-01', highlights: 'A release.' };
const expected = {
  badge: 'release-v1.2.3-blue', install: 'npm install -g locus-cli@1.2.3 @ashlrai/locus-mcp@1.2.3',
  versions: '| Rust workspace | 1.2.3 |', pin: 'npx --yes @ashlrai/locus-mcp@1.2.3 --version',
  release: '(2026-01-01).\n\nA release.',
};
for (const [name, text] of Object.entries(expected)) {
  test(`${name} renders release sources and preserves surrounding text`, () => {
    const input = `before\n<!-- fresh:begin ${name} -->\nstale\n<!-- fresh:end -->\nafter`;
    const result = replaceBlocks(input, [name], config.templates, values);
    assert.ok(result.includes(text));
    assert.ok(result.startsWith('before\n'));
    assert.ok(result.endsWith('\nafter'));
    assert.equal(replaceBlocks(result, [name], config.templates, values), result);
  });
}
test('missing, unknown, duplicate and malformed blocks fail closed', () => {
  for (const input of ['', '<!-- fresh:begin nope -->\nx\n<!-- fresh:end -->',
    '<!-- fresh:begin badge -->\nx\n<!-- fresh:end -->'.repeat(2),
    '<!-- fresh:begin badge -->\nx']) {
    assert.throws(() => replaceBlocks(input, ['badge'], config.templates, values));
  }
});
test('CLI rejects seeded drift, sync repairs it, source disagreement writes nothing', async () => {
  const root = await mkdtemp(join(tmpdir(), 'locus-freshness-'));
  try {
    for (const path of ['Cargo.toml', 'npm/package.json', 'npm-mcp/package.json', 'scripts/freshness', ...Object.keys(config.files)]) {
      await cp(join(repository, path), join(root, path), { recursive: true });
    }
    const cli = name => spawnSync(process.execPath, [join(root, `scripts/freshness/${name}.mjs`)], { encoding: 'utf8' });
    assert.equal(cli('check').status, 0);
    const path = join(root, 'README.md');
    const original = await readFile(path, 'utf8');
    await writeFile(path, original.replace('release-v', 'drift-v'));
    assert.equal(cli('check').status, 1);
    assert.match(await readFile(path, 'utf8'), /drift-v/);
    assert.equal(cli('sync').status, 0);
    assert.equal(cli('check').status, 0);
    assert.equal(await readFile(path, 'utf8'), original);
    assert.deepEqual(await run(root), []);
    const currentVersion = (await sources(root)).version;
    // A release bump regenerates every registered surface from the sources.
    for (const source of ['Cargo.toml', 'npm/package.json', 'npm-mcp/package.json', 'CHANGELOG.md']) {
      const sourcePath = join(root, source);
      const text = await readFile(sourcePath, 'utf8');
      await writeFile(sourcePath, text.replaceAll(currentVersion, '9.8.7'));
    }
    assert.equal(cli('check').status, 1);
    assert.equal(cli('sync').status, 0);
    assert.equal(cli('check').status, 0);
    for (const file of Object.keys(config.files)) {
      assert.match(await readFile(join(root, file), 'utf8'), /9\.8\.7/);
    }
    const bumped = await readFile(path, 'utf8');
    await writeFile(join(root, 'npm/package.json'), '{"version":"9.9.9"}');
    await assert.rejects(sources(root), /disagree/);
    await assert.rejects(run(root), /disagree/);
    assert.equal(await readFile(path, 'utf8'), bumped);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
