import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { anchorsOf, checkMarkdown, collectTargets, repository, slugify } from './check-readme-links.mjs';

const README = join(repository, 'README.md');
const CLI = join(repository, 'scripts/check-readme-links.mjs');

test('repository README: every link resolves', async () => {
  assert.deepEqual(await checkMarkdown(README), { failures: [], warnings: [] });
});

test('README distinguishes the proposed shell installer from current options', async () => {
  const readme = await readFile(README, 'utf8');
  const install = readme.split('## Install options\n')[1].split('\n## ', 1)[0];
  const [current, proposal] = install.split('### Proposed shell installer\n');
  assert.ok(proposal, 'installer proposal is missing');
  assert.match(current, /brew install ashlrai\/tap\/locus/);
  assert.match(current, /npm install -g locus-cli @ashlrai\/locus-mcp/);
  assert.match(proposal, /Issue #169/);
  assert.match(proposal, /does not establish that a published one-command installation path/);
  assert.match(proposal, /locus-<target>\.tar\.gz/);
  assert.match(proposal, /\.sha256/);
  assert.match(proposal, /Missing or mismatched checksums must stop/);
  assert.match(proposal, /macOS Apple Silicon and\s+Intel, and Linux x86_64 with glibc/);
  assert.match(proposal, /unsupported platforms, including Linux\s+ARM/);
  assert.match(proposal, /### Install from source/);
});

test('README install investigation reports only core artifacts and freshness', async () => {
  const readme = await readFile(README, 'utf8');
  const script = readme.match(/python3 - "\$investigation_dir\/artifacts\.jsonl" <<'PY'\n([\s\S]*?)\nPY/);
  assert.ok(script, 'README investigation Python command is missing');
  const dir = await mkdtemp(join(tmpdir(), 'locus-artifacts-'));
  try {
    const path = join(dir, 'artifacts.jsonl');
    const artifact = (name, fresh, filename) => ({
      reason: 'compiler-artifact', package_id: 'locus-core 0.5.2',
      target: { name, kind: ['lib'] }, profile: { opt_level: '3' },
      features: [], filenames: [filename], fresh,
    });
    await writeFile(path, [
      { reason: 'build-finished' },
      artifact('other', false, '/tmp/other.rlib'),
      artifact('locus_core', false, '/tmp/liblocus_core-first.rlib'),
      artifact('locus_core', true, '/tmp/liblocus_core-second.rlib'),
    ].map(JSON.stringify).join('\n') + '\n');
    const run = spawnSync('python3', ['-c', script[1], path], { encoding: 'utf8' });
    assert.equal(run.status, 0, run.stderr);
    const lines = run.stdout.trim().split('\n');
    assert.equal(lines.length, 2);
    assert.match(lines[0], /liblocus_core-first\.rlib.*'fresh': False/);
    assert.match(lines[1], /liblocus_core-second\.rlib.*'fresh': True/);
    assert.doesNotMatch(run.stdout, /other\.rlib|\/tmp\//);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('contributor guide: every link resolves', async () => {
  assert.deepEqual(await checkMarkdown(join(repository, 'CONTRIBUTING.md')), { failures: [], warnings: [] });
});

test('CLI exits 0 on the repository README', () => {
  const run = spawnSync(process.execPath, [CLI], { encoding: 'utf8' });
  assert.equal(run.status, 0, run.stderr);
  assert.match(run.stdout, /ok /);
});

test('slugify matches GitHub heading anchors', () => {
  assert.equal(slugify('30-second start'), '30-second-start');
  assert.equal(slugify('MCP (Claude Code / Cursor)'), 'mcp-claude-code--cursor');
  assert.equal(slugify('Supervised launch (provider tools)'), 'supervised-launch-provider-tools');
  assert.equal(anchorsOf('# A\n# A\n# B!').has('a-1'), true);
});

test('collectTargets covers markdown links, html attrs and fenced bare URLs', () => {
  const md = [
    '[a](./x.md) ![i](./i.png) <img src="./og.png">',
    '```bash',
    'curl https://example.com/install.sh | sh',
    '```',
  ].join('\n');
  const targets = collectTargets(md);
  for (const t of ['./x.md', './i.png', './og.png', 'https://example.com/install.sh']) {
    assert.ok(targets.includes(t), `missing ${t}`);
  }
});

test('missing files, missing anchors and malformed URLs fail closed', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'locus-links-'));
  try {
    await writeFile(join(dir, 'exists.md'), '# Target\n\n## Section One\n');
    const cases = [
      ['[x](./nope.md)', /missing file/],
      ['[x](#no-such-heading)', /missing anchor #no-such-heading/],
      ['[x](./exists.md#other-section)', /missing anchor #other-section/],
      ['[x](http://[)', /malformed URL/],
      ['<img src="./missing.png">', /missing file/],
    ];
    for (const [line, pattern] of cases) {
      const path = join(dir, 'case.md');
      await writeFile(path, `# Case\n\n${line}\n`);
      const { failures } = await checkMarkdown(path);
      assert.equal(failures.length, 1, `${line}: ${failures}`);
      assert.match(failures[0], pattern);
    }
    // Anchor into a sibling file does resolve.
    const path = join(dir, 'ok.md');
    await writeFile(path, '[x](./exists.md#section-one)\n');
    assert.deepEqual(await checkMarkdown(path), { failures: [], warnings: [] });
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('CLI reports each failure and exits 1', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'locus-links-cli-'));
  try {
    const path = join(dir, 'bad.md');
    await writeFile(path, '[x](./nope.md)\n[y](#gone)\n');
    const run = spawnSync(process.execPath, [CLI, path], { encoding: 'utf8' });
    assert.equal(run.status, 1);
    assert.match(run.stderr, /missing file: \.\/nope\.md/);
    assert.match(run.stderr, /missing anchor #gone/);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
