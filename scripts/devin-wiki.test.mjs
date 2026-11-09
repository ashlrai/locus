// devin-wiki.test.mjs — keep .devin/wiki.json (the DeepWiki auto-docs config)
// valid and fresh. DeepWiki generates exactly the pages listed there, so a
// stale title, a broken parent link, or a path that no longer exists degrades
// the generated wiki silently. Contract and limits per the DeepWiki docs:
// both top-level fields required (repo_notes may be empty), at least one
// page, max 30 pages, max 100 notes total, max 10,000 characters per note.
import test from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { repository } from './check-readme-links.mjs';

const wiki = JSON.parse(readFileSync(join(repository, '.devin/wiki.json'), 'utf8'));
const pages = wiki.pages ?? [];
const notes = [...(wiki.repo_notes ?? []), ...pages.flatMap((page) => page.page_notes ?? [])];

test('.devin/wiki.json satisfies the DeepWiki config contract', () => {
  assert.ok(Array.isArray(wiki.repo_notes), 'repo_notes must be an array (empty is allowed)');
  assert.ok(Array.isArray(pages) && pages.length > 0, 'pages must list at least one page');
  assert.ok(pages.length <= 30, `expected at most 30 pages, found ${pages.length}`);
  assert.ok(notes.length <= 100, `expected at most 100 notes, found ${notes.length}`);

  for (const note of notes) {
    assert.equal(typeof note, 'object', `note must be an object: ${JSON.stringify(note)}`);
    assert.ok(typeof note.content === 'string' && note.content.length > 0, 'every note needs content');
    assert.ok(note.content.length <= 10_000, `note exceeds 10,000 characters (${note.content.length})`);
    assert.ok(note.author === undefined || typeof note.author === 'string', 'note author must be a string when present');
  }

  const titles = pages.map((page) => page.title);
  for (const page of pages) {
    assert.ok(typeof page.title === 'string' && page.title.length > 0, 'every page needs a title');
    assert.ok(typeof page.purpose === 'string' && page.purpose.length > 0, `page ${page.title} needs a purpose`);
  }
  assert.equal(new Set(titles).size, titles.length, 'page titles must be unique');
});

test('every parent link resolves to another page and the hierarchy terminates', () => {
  const titles = new Set(pages.map((page) => page.title));
  for (const page of pages) {
    if (page.parent === null || page.parent === undefined) continue;
    assert.ok(titles.has(page.parent), `page ${page.title} names unknown parent ${page.parent}`);
    assert.notEqual(page.parent, page.title, `page ${page.title} parents itself`);
  }
  for (const start of pages) {
    const seen = new Set();
    let current = start;
    while (current?.parent) {
      assert.ok(!seen.has(current.title), `parent cycle through ${current.title}`);
      seen.add(current.title);
      current = pages.find((page) => page.title === current.parent);
    }
  }
});

test('every repository path named by the wiki config exists', () => {
  const prose = [
    ...notes.map((note) => note.content ?? ''),
    ...pages.flatMap((page) => [page.title ?? '', page.purpose ?? '', ...(page.page_notes ?? []).map((note) => note.content ?? '')]),
  ].join('\n');
  const prefixed = prose.match(/(?:crates|docs|scripts|examples|adapters|apps|npm|npm-mcp|schema|eval|integrations|tests|\.devin)\/[A-Za-z0-9._/-]*/g) ?? [];
  const rooted = prose.match(/\b(?:README|AGENTS|CLAUDE|CONTRIBUTING|SECURITY|DESIGN|PLAN|GOALS|SUPPORT|CODE_OF_CONDUCT|CHANGELOG)\.md\b|\bllms(?:-full)?\.txt\b/g) ?? [];
  const paths = new Set([...prefixed, ...rooted].map((path) => path.replace(/[.,;:]+$/, '')));
  assert.ok(paths.size > 0, 'expected the config to reference repository paths');
  for (const path of paths) {
    assert.ok(existsSync(join(repository, path)), `referenced path is gone: ${path}`);
  }
});

test('the DeepWiki config is documented for contributors', () => {
  const contributing = readFileSync(join(repository, 'CONTRIBUTING.md'), 'utf8');
  assert.match(contributing, /devin-wiki\.test\.mjs/, 'CONTRIBUTING.md lists the check');
  const docsIndex = readFileSync(join(repository, 'docs/README.md'), 'utf8');
  assert.match(docsIndex, /\[deepwiki\.md\]\(deepwiki\.md\)/, 'docs/README.md links the guide');
  const guide = readFileSync(join(repository, 'docs/deepwiki.md'), 'utf8');
  for (const marker of [/repo_notes/, /"pages"|`pages`/, /\.devin\/wiki\.json/, /30 pages/]) {
    assert.match(guide, marker);
  }
});
