import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { repository } from './check-readme-links.mjs';

const SCRIPT = join(repository, 'scripts/check-support-links.sh');
const SUPPORT = join(repository, 'SUPPORT.md');

async function withFixture(files, body) {
  const dir = await mkdtemp(join(tmpdir(), 'locus-support-'));
  try {
    for (const [name, text] of Object.entries(files)) {
      await mkdir(join(dir, name, '..'), { recursive: true });
      await writeFile(join(dir, name), text);
    }
    return await body(dir);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
}

function runScript(file, env = {}) {
  return spawnSync('bash', [SCRIPT, ...(file ? [file] : [])], {
    encoding: 'utf8',
    env: { ...process.env, ...env },
  });
}

test('repository SUPPORT.md: every relative link resolves', () => {
  const run = runScript();
  assert.equal(run.status, 0, `${run.stdout}\n${run.stderr}`);
  assert.match(run.stdout, /check-support-links: ok \(\d+ relative links/);
});

test('repository SUPPORT.md is linked from the README docs table', async () => {
  const readme = await readFile(join(repository, 'README.md'), 'utf8');
  assert.match(readme, /\[SUPPORT\.md\]\(\.\/SUPPORT\.md\)/);
});

test('SUPPORT.md names the security policy, contributing guide and code of conduct', async () => {
  const support = await readFile(SUPPORT, 'utf8');
  for (const target of ['./SECURITY.md', './CONTRIBUTING.md', './CODE_OF_CONDUCT.md']) {
    assert.ok(support.includes(`](${target})`), `missing link to ${target}`);
  }
});

test('temp copy with a broken link exits 1 and names the target', async () => {
  await withFixture({ 'SUPPORT.md': '# Help\n\n- [Gone](./missing/guide.md)\n' }, (dir) => {
    const run = runScript(join(dir, 'SUPPORT.md'));
    assert.equal(run.status, 1, `${run.stdout}\n${run.stderr}`);
    assert.match(run.stderr, /broken link in .*SUPPORT\.md:3: \.\/missing\/guide\.md/);
    assert.match(run.stderr, /1 broken of 1 relative links/);
  });
});

test('all-resolving fixture exits 0 across inline, image, reference and title forms', async () => {
  const files = {
    'SUPPORT.md': [
      '# Help',
      '',
      '[a](./docs/a.md) ![logo](./img/logo.png "Logo") [b](<./docs/b c.md>)',
      '[d](docs/) [e](./docs/a.md#section) [f](./docs/a.md?raw=1)',
      '',
      '[ref]: ./docs/ref.md "Ref title"',
      '',
    ].join('\n'),
    'docs/a.md': '# A\n',
    'docs/b c.md': '# B\n',
    'docs/ref.md': '# R\n',
    'img/logo.png': 'png',
  };
  await withFixture(files, (dir) => {
    const run = runScript(join(dir, 'SUPPORT.md'));
    assert.equal(run.status, 0, `${run.stdout}\n${run.stderr}`);
    assert.match(run.stdout, /7 relative links/);
  });
});

test('relative links resolve next to the file; /-prefixed links resolve from the repository root', async () => {
  await withFixture({
    'SUPPORT.md': '[root](/SECURITY.md)\n[nested](./guide/index.md)\n',
    'guide/index.md': '# g\n',
  }, (dir) => {
    assert.equal(runScript(join(dir, 'SUPPORT.md')).status, 0);
  });
  await withFixture({ 'SUPPORT.md': '[nope](/definitely-not-here.md)\n' }, (dir) => {
    assert.equal(runScript(join(dir, 'SUPPORT.md')).status, 1);
  });
  // The file sits at the fixture root, so ./guide/index.md must not fall back to the repository root.
  await withFixture({ 'SUPPORT.md': '[wrong](./guide/index.md)\n', 'index.md': '# i\n' }, (dir) => {
    assert.equal(runScript(join(dir, 'SUPPORT.md')).status, 1);
  });
});

test('external schemes, protocol-relative URLs, same-file anchors and fenced code are not checked', async () => {
  const support = [
    '# Help',
    '[site](https://example.com/help)',
    '[mail](mailto:help@example.com)',
    '[proto](//cdn.example.com/x.js)',
    '[top](#help)',
    '[empty]()',
    '',
    '```markdown',
    '[inside](./does-not-exist.md)',
    '```',
    '',
    '~~~',
    '[inside-tilde](./also-missing.md)',
    '~~~',
    '',
    '[colon-in-name](./notes/2026:11.md)',
  ].join('\n');
  await withFixture({ 'SUPPORT.md': support, 'notes/2026:11.md': 'n\n' }, (dir) => {
    const run = runScript(join(dir, 'SUPPORT.md'));
    assert.equal(run.status, 0, `${run.stdout}\n${run.stderr}`);
    assert.match(run.stdout, /1 relative links/);
  });
});

test('unreadable or missing file exits 2, not 0', () => {
  const run = runScript(join(repository, 'does-not-exist', 'SUPPORT.md'));
  assert.equal(run.status, 2);
  assert.match(run.stderr, /cannot read/);
});

test('a fixture with no relative links exits 0 and reports zero checked', async () => {
  await withFixture({ 'SUPPORT.md': '# Help\n\nEmail [us](mailto:help@example.com).\n' }, (dir) => {
    const run = runScript(join(dir, 'SUPPORT.md'));
    assert.equal(run.status, 0, run.stderr);
    assert.match(run.stdout, /\(0 relative links/);
  });
});
