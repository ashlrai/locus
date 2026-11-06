import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { test } from 'node:test';
import { buildInputs, buildOutputs, docsIndex, readmeInfo, securityContact } from './discoverability.mjs';

function fixture(t, files) {
  const root = mkdtempSync(join(tmpdir(), 'discoverability-sonar-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  for (const [name, text] of Object.entries(files)) {
    const path = join(root, name);
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, text);
  }
  return root;
}

// A subprocess deadline can interrupt a synchronous regex regression;
// node:test's in-process timeout cannot interrupt a blocked event loop.
function boundedCheck(root, source) {
  const module = new URL('./discoverability.mjs', import.meta.url).href;
  const result = spawnSync(process.execPath, ['--input-type=module', '-e', `
    import assert from 'node:assert/strict';
    import { readmeInfo, docsIndex, securityContact } from ${JSON.stringify(module)};
    const root = ${JSON.stringify(root)};
    ${source}
  `], { encoding: 'utf8', timeout: 5000 });
  assert.ifError(result.error);
  assert.equal(result.status, 0, result.stderr);
}

test('H1 preserves first match, greedy whitespace, line endings and empty cleaned titles', (t) => {
  for (const [text, title] of [
    ['#  Locus — identity  \r\n# ignored', 'Locus - identity'],
    ['# \nword', 'word'], ['#  \n', ''], ['#\r\n \n', ''], ['# \u2028\t', ''],
  ]) assert.equal(readmeInfo(fixture(t, { 'README.md': text })).title, title);
  for (const text of ['# \n', '## no H1', '#no space']) {
    assert.throws(() => readmeInfo(fixture(t, { 'README.md': text })), /no H1/);
  }
});

test('H1 preserves a long whitespace-only heading within a bounded process', (t) => {
  boundedCheck(fixture(t, { 'README.md': `#${' '.repeat(200_000)}\n` }),
    "assert.equal(readmeInfo(root).title, '')");
  boundedCheck(fixture(t, { 'README.md': `#${'\n'.repeat(200_000)}` }),
    'assert.throws(() => readmeInfo(root), /no H1/)');
});

test('H2 preserves greedy whitespace, empty sections and heading boundaries', (t) => {
  const root = fixture(t, { 'README.md': '# Locus\n##  Agent — tools  \r\n### excluded\n##no space\n## \nnext\n##  \n' });
  assert.deepEqual(readmeInfo(root).sections, ['Agent - tools', 'next', '']);
});

test('H2 preserves a pathological whitespace section', (t) => {
  boundedCheck(fixture(t, { 'README.md': `# Locus\n##${' '.repeat(200_000)}\n` }),
    "assert.deepEqual(readmeInfo(root).sections, [''])");
  boundedCheck(fixture(t, { 'README.md': `# Locus\n##${'\n'.repeat(200_000)}` }),
    'assert.deepEqual(readmeInfo(root).sections, [])');
});

test('docs heading regex keeps levels, multiline whitespace, markup and empty filtering', (t) => {
  const root = fixture(t, { 'docs/fixture.md': '# [Doc] _title_\r\n##  Two — `code`\n### \nnext\n#### excluded\n##no space\n##  \n' });
  assert.deepEqual(docsIndex(root), [{ file: 'fixture.md', title: 'Doc title', headings: [
    { level: 2, text: 'Two - code' }, { level: 3, text: 'next' },
  ] }]);
});

test('docs heading regex filters pathological empty headings and keeps filename fallback', (t) => {
  boundedCheck(fixture(t, { 'docs/fixture.md': `###${' '.repeat(200_000)}\n` }),
    "assert.deepEqual(docsIndex(root), [{ file: 'fixture.md', title: 'fixture', headings: [] }])");
  boundedCheck(fixture(t, { 'docs/fixture.md': `###${'\n'.repeat(200_000)}` }),
    "assert.deepEqual(docsIndex(root), [{ file: 'fixture.md', title: 'fixture', headings: [] }])");
});

test('trailing hash regex removes only a terminal run, before trimming whitespace', (t) => {
  const root = fixture(t, { 'docs/fixture.md': '# Doc###\n## Two###\n## Three### \n## Four###x\n## #####\n' });
  assert.deepEqual(docsIndex(root)[0], { file: 'fixture.md', title: 'Doc', headings: [
    { level: 2, text: 'Two' }, { level: 2, text: 'Three###' }, { level: 2, text: 'Four###x' },
  ] });
});

test('trailing hash regex keeps a long nonterminal run and removes a terminal run', (t) => {
  const hashes = '#'.repeat(200_000);
  boundedCheck(fixture(t, { 'docs/fixture.md': `# Doc\n## ${hashes}x\n## kept${hashes}\n` }), `
    assert.deepEqual(docsIndex(root)[0].headings, [
      { level: 2, text: '#'.repeat(200_000) + 'x' }, { level: 2, text: 'kept' },
    ]);
  `);
});

test('email regex preserves first match, casing and permissive domain syntax', (t) => {
  for (const [text, expected] of [
    ['mailto:Sec_urity+tag%.-@EXAMPLE..co.uk then later@example.org', 'Sec_urity+tag%.-@EXAMPLE..co.uk'],
    ['bad@x.c valid@example.org', 'valid@example.org'], ['a@b@c.org', 'b@c.org'],
    ['a@b.co123', 'a@b.co'], ['a@b.-.com', 'a@b.-.com'],
  ]) assert.equal(securityContact(fixture(t, { 'SECURITY.md': text })), expected);
  assert.throws(() => securityContact(fixture(t, { 'SECURITY.md': 'a@b.c' })), /no contact email/);
});

test('email regex handles long local parts, invalid domains and repeated dots', (t) => {
  for (const text of ['a'.repeat(200_000), `a@${'b'.repeat(200_000)}`, `a@${'b.'.repeat(100_000)}x`]) {
    boundedCheck(fixture(t, { 'SECURITY.md': text }),
      'assert.throws(() => securityContact(root), /no contact email/)');
  }
  boundedCheck(fixture(t, { 'SECURITY.md': `${'a'.repeat(200_000)}@b.co` }),
    "assert.equal(securityContact(root), 'a'.repeat(200_000) + '@b.co')");
});

test('S930: buildOutputs emits the fixed robots policy without renderer arguments', () => {
  const inputs = buildInputs();
  const expected = '# Locus - identity plane for coding agents\n# Agents: see /llms.txt, /capabilities.json and /server.json\nUser-agent: *\nAllow: /\n\nSitemap: https://locus.dev/sitemap.xml\n';
  assert.equal(buildOutputs(undefined, inputs).files['robots.txt'], expected);
  assert.equal(buildOutputs(undefined, { ...inputs, version: '999.0.0', pages: [] }).files['robots.txt'], expected);
});
