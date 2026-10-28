// Acceptance + behavior tests for scripts/resolve-merge-conflicts.mjs and the
// `.gitattributes` union driver. Every fixture is a real `git init` in a
// throwaway directory with a throwaway HOME; resolution is pure git plus the
// workspace's own package managers — no model call anywhere.
//
// Run: node --test scripts/resolve-merge-conflicts.test.mjs
import test from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { homedir, tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { classify } from './resolve-merge-conflicts.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const REPO = resolve(here, '..');
const SCRIPT = join(here, 'resolve-merge-conflicts.mjs');
const GITATTR = readFileSync(join(REPO, '.gitattributes'), 'utf8');

// Hermetic child env: fake HOME, no user gitconfig, fixture identity via env,
// toolchain paths (CARGO_HOME/RUSTUP_HOME) inherited, cargo kept offline —
// every fixture uses only path deps so nothing needs a registry.
const FAKE_HOME = mkdtempSync(join(tmpdir(), 'locus-test-home-'));
const ENV = {
  ...process.env,
  HOME: FAKE_HOME,
  GIT_CONFIG_NOSYSTEM: '1',
  GIT_CONFIG_GLOBAL: join(FAKE_HOME, 'gitconfig'),
  GIT_AUTHOR_NAME: 'fixture', GIT_AUTHOR_EMAIL: 'fixture@example.test',
  GIT_COMMITTER_NAME: 'fixture', GIT_COMMITTER_EMAIL: 'fixture@example.test',
  GIT_EDITOR: 'true', GIT_SEQUENCE_EDITOR: 'true',
  CARGO_NET_OFFLINE: 'true',
  CARGO_HOME: process.env.CARGO_HOME ?? join(homedir(), '.cargo'),
  RUSTUP_HOME: process.env.RUSTUP_HOME ?? join(homedir(), '.rustup'),
};

const has = (bin) => spawnSync(bin, ['--version'], { env: ENV }).status === 0;

function git(dir, args, { check = true } = {}) {
  const r = spawnSync('git', args, { cwd: dir, env: ENV, encoding: 'utf8' });
  if (check && r.status !== 0) throw new Error(`git ${args.join(' ')}: ${r.stderr || r.stdout}`);
  return r;
}
const sh = (dir, cmd, args) => {
  const r = spawnSync(cmd, args, { cwd: dir, env: ENV, encoding: 'utf8' });
  if (r.status !== 0) throw new Error(`${cmd} ${args.join(' ')}: ${r.stderr || r.stdout}`);
  return r;
};
const script = (dir, args) =>
  spawnSync(process.execPath, [SCRIPT, ...args], { cwd: dir, env: ENV, encoding: 'utf8' });

function put(dir, rel, content) {
  const abs = join(dir, rel);
  mkdirSync(dirname(abs), { recursive: true });
  writeFileSync(abs, content);
}
const read = (dir, rel) => readFileSync(join(dir, rel), 'utf8');
const commitAll = (dir, msg) => { git(dir, ['add', '-A']); git(dir, ['commit', '-q', '-m', msg]); };
const unmerged = (dir) => git(dir, ['diff', '--name-only', '--diff-filter=U']).stdout.trim().split('\n').filter(Boolean);
const assertNoMarkers = (text) => assert.doesNotMatch(text, /^(<{7}|>{7}|\|{7}|={7}$)/m, 'conflict markers remained');
const assertClean = (dir) => assert.equal(git(dir, ['status', '--porcelain']).stdout.trim(), '', 'worktree not clean');

function newRepo(files) {
  const dir = mkdtempSync(join(tmpdir(), 'locus-conflict-'));
  git(dir, ['init', '-q', '-b', 'main']);
  for (const [p, c] of Object.entries(files)) put(dir, p, c);
  commitAll(dir, 'base');
  return dir;
}

// Branch `pr` edits `files` on top of base; main edits `mainFiles`; then `pr`
// rebases onto main. Returns the rebase spawn result.
function rebaseFixture(dir, prFiles, mainFiles) {
  git(dir, ['checkout', '-q', '-b', 'pr']);
  for (const [p, c] of Object.entries(prFiles)) put(dir, p, c);
  commitAll(dir, 'pr changes');
  git(dir, ['checkout', '-q', 'main']);
  for (const [p, c] of Object.entries(mainFiles)) put(dir, p, c);
  commitAll(dir, 'main changes');
  git(dir, ['checkout', '-q', 'pr']);
  return git(dir, ['rebase', 'main'], { check: false });
}

const CHANGELOG_BASE = '# Changelog\n\n## [Unreleased]\n\n## [1.0.0] - 2026-01-01\n\n- shipped\n';
const changelogPr = CHANGELOG_BASE.replace('\n## [1.0.0]', '- pr adds pin alias docs\n\n## [1.0.0]');
const changelogMain = CHANGELOG_BASE.replace('\n## [1.0.0]', '- main fixes doctor exit code\n\n## [1.0.0]');

// ---------- .gitattributes guard (SEC amendment: only merge=union on CHANGELOG paths) ----------

test('.gitattributes adds only merge=union lines scoped to CHANGELOG paths', () => {
  const lines = GITATTR.split('\n').map((l) => l.trim()).filter((l) => l && !l.startsWith('#'));
  assert.ok(lines.length > 0, '.gitattributes is empty');
  for (const line of lines) {
    assert.match(line, /^[^\s]+\s+merge=union$/, `not a scoped union line: ${line}`);
    assert.match(line.split(/\s+/)[0], /CHANGELOG/i, `not a CHANGELOG path: ${line}`);
    assert.doesNotMatch(line, /binary|-diff|-text|filter=|(?<!\w)diff=/, `forbidden attribute: ${line}`);
  }
});

test('classify maps only governed paths to policies', () => {
  assert.equal(classify('CHANGELOG.md'), 'union');
  assert.equal(classify('docs/guide/CHANGELOG.md'), 'union');
  assert.equal(classify('CLAUDE.md'), 'doc-ours-plus-incoming');
  assert.equal(classify('README.md'), 'doc-ours-plus-incoming');
  assert.equal(classify('Cargo.lock'), 'cargo-lock');
  assert.equal(classify('bun.lock'), 'bun-lock');
  assert.equal(classify('bun.lockb'), 'bun-lock');
  assert.equal(classify('package-lock.json'), 'npm-lock');
  assert.equal(classify('Cargo.toml'), null);
  assert.equal(classify('src/lib.rs'), null);
  assert.equal(classify('.gitattributes'), null);
});

// ---------- acceptance: rebase a fixture CHANGELOG conflict cleanly, no model call ----------

test('git rebase resolves a fixture CHANGELOG.md conflict via the union driver', () => {
  const dir = newRepo({ '.gitattributes': GITATTR, 'CHANGELOG.md': CHANGELOG_BASE });
  const r = rebaseFixture(dir, { 'CHANGELOG.md': changelogPr }, { 'CHANGELOG.md': changelogMain });
  assert.equal(r.status, 0, `rebase should not stop: ${r.stderr || r.stdout}`);
  const out = read(dir, 'CHANGELOG.md');
  assert.match(out, /pr adds pin alias docs/);
  assert.match(out, /main fixes doctor exit code/);
  assertNoMarkers(out);
  assertClean(dir);
});

test('without the attribute the same fixture conflicts; the helper unions it anyway', () => {
  const dir = newRepo({ 'CHANGELOG.md': CHANGELOG_BASE }); // no .gitattributes
  const r = rebaseFixture(dir, { 'CHANGELOG.md': changelogPr }, { 'CHANGELOG.md': changelogMain });
  assert.notEqual(r.status, 0, 'expected a real conflict without the driver');
  assert.deepEqual(unmerged(dir), ['CHANGELOG.md']);

  const s = script(dir, ['--continue']);
  assert.equal(s.status, 0, s.stdout + s.stderr);
  const out = read(dir, 'CHANGELOG.md');
  assert.match(out, /pr adds pin alias docs/);
  assert.match(out, /main fixes doctor exit code/);
  assertNoMarkers(out);
  assertClean(dir);
});

// ---------- doc policy: target's version + incoming hunks ----------

const docBase = (title, word) => [`# ${title}`, ...Array.from({ length: 9 }, (_, i) => `${word} ${i + 1}`)].join('\n') + '\n';

test('helper resolves CLAUDE.md and README.md as target version plus incoming hunks', () => {
  const claudeBase = docBase('CLAUDE', 'guide');
  const readmeBase = docBase('README', 'intro');
  const dir = newRepo({
    '.gitattributes': GITATTR,
    'CLAUDE.md': claudeBase,
    'README.md': readmeBase,
  });
  const r = rebaseFixture(
    dir,
    {
      // pr rewrites line 2 (collides with main) and line 9 (clean, must apply)
      'CLAUDE.md': claudeBase.replace('guide 1', 'pr heading blurb').replace('guide 8', 'pr adds guidance'),
      'README.md': readmeBase.replace('intro 1', 'pr heading blurb').replace('intro 8', 'pr adds readme section'),
    },
    {
      'CLAUDE.md': claudeBase.replace('guide 1', 'main heading blurb'),
      'README.md': readmeBase.replace('intro 1', 'main heading blurb'),
    },
  );
  assert.notEqual(r.status, 0, 'expected the doc files to conflict');
  assert.deepEqual(unmerged(dir).sort(), ['CLAUDE.md', 'README.md']);

  const s = script(dir, ['--continue']);
  assert.equal(s.status, 0, s.stdout + s.stderr);
  for (const [file, mainWon, prKept] of [
    ['CLAUDE.md', 'main heading blurb', 'pr adds guidance'],
    ['README.md', 'main heading blurb', 'pr adds readme section'],
  ]) {
    const out = read(dir, file);
    assert.match(out, new RegExp(mainWon), `${file}: target's text must win the overlap`);
    assert.match(out, new RegExp(prKept), `${file}: incoming clean hunk must be re-applied`);
    assert.doesNotMatch(out, /pr heading blurb/, `${file}: colliding incoming hunk must not survive`);
    assertNoMarkers(out);
  }
  assertClean(dir);
});

// ---------- lockfile policies: regenerated by the package manager ----------

const appToml = (dep) => `[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\nshared = { path = "shared" }${dep}`;
const krate = (name) => ({
  [`${name}/Cargo.toml`]: `[package]\nname = "${name}"\nversion = "0.1.0"\nedition = "2021"\n`,
  [`${name}/src/lib.rs`]: `// ${name}\n`,
});

test('Cargo.lock conflicts are regenerated by cargo, never hand-merged', { skip: !has('cargo') && 'cargo not on PATH' }, () => {
  const dir = newRepo({
    '.gitattributes': GITATTR,
    'Cargo.toml': appToml('\n'),
    'src/lib.rs': '// app\n',
    ...krate('shared'),
  });
  sh(dir, 'cargo', ['generate-lockfile']);
  commitAll(dir, 'base lockfile');

  // pr adds a path dep (regenerates the lockfile); main appends a stale
  // package block at the same end-of-file region — a real lockfile conflict.
  git(dir, ['checkout', '-q', '-b', 'pr']);
  put(dir, 'Cargo.toml', appToml('\nzebra = { path = "zebra" }\n'));
  for (const [p, c] of Object.entries(krate('zebra'))) put(dir, p, c);
  sh(dir, 'cargo', ['generate-lockfile']);
  commitAll(dir, 'add zebra dep');
  git(dir, ['checkout', '-q', 'main']);
  put(dir, 'Cargo.lock', read(dir, 'Cargo.lock') + '[[package]]\nname = "stale"\nversion = "9.9.9"\n');
  commitAll(dir, 'stale lockfile entry');
  git(dir, ['checkout', '-q', 'pr']);

  const r = git(dir, ['rebase', 'main'], { check: false });
  assert.notEqual(r.status, 0, 'expected Cargo.lock to conflict');
  assert.deepEqual(unmerged(dir), ['Cargo.lock']);

  const s = script(dir, ['--continue']);
  assert.equal(s.status, 0, s.stdout + s.stderr);
  const lock = read(dir, 'Cargo.lock');
  assertNoMarkers(lock);
  assert.match(lock, /name = "zebra"/, 'cargo did not re-resolve the new dep');
  assert.match(read(dir, 'Cargo.toml'), /zebra = \{ path = "zebra" \}/, 'manifest must be untouched');
  assert.equal(sh(dir, 'cargo', ['metadata', '--locked', '--quiet']).status, 0, 'regenerated lockfile is not frozen-consistent');
  assertClean(dir);
});

test('bun.lock conflicts are regenerated by bun, never hand-merged', { skip: !has('bun') && 'bun not on PATH' }, () => {
  // A local file: dep keeps `bun install` offline and makes it emit bun.lock.
  const dir = newRepo({
    '.gitignore': 'node_modules/\n',
    'package.json': '{"name":"fixt","version":"1.0.0","dependencies":{"loc":"file:./loc"}}\n',
    'loc/package.json': '{"name":"loc","version":"1.0.0"}\n',
  });
  sh(dir, 'bun', ['install']);
  assert.ok(existsSync(join(dir, 'bun.lock')), 'bun did not emit bun.lock');
  commitAll(dir, 'base lockfile');

  // Both sides rewrite the workspace name line in the lockfile — same hunk,
  // guaranteed conflict. Regeneration restores it from package.json.
  const base = read(dir, 'bun.lock');
  const nameLine = base.split('\n').find((l) => l.includes('"name"'));
  assert.ok(nameLine, 'bun.lock has no workspace name line to conflict on');
  const named = (v) => base.replace(nameLine, nameLine.replace(/"name"\s*:\s*"[^"]*"/, `"name": "${v}"`));

  const r = rebaseFixture(dir, { 'bun.lock': named('fixt-pr') }, { 'bun.lock': named('fixt-main') });
  assert.notEqual(r.status, 0, 'expected bun.lock to conflict');
  assert.deepEqual(unmerged(dir), ['bun.lock']);

  const s = script(dir, ['--continue']);
  assert.equal(s.status, 0, s.stdout + s.stderr);
  const lock = read(dir, 'bun.lock');
  assertNoMarkers(lock);
  assert.match(lock, /"loc"/, 'bun install dropped the file: dependency');
  assert.equal(sh(dir, 'bun', ['install', '--frozen-lockfile']).status, 0, 'regenerated lockfile is not frozen-consistent');
  assertClean(dir);
});

test('--continue works when invoked from a subdirectory of the repo', () => {
  const dir = newRepo({ 'CLAUDE.md': docBase('CLAUDE', 'guide'), 'sub/.keep': '' });
  const base = read(dir, 'CLAUDE.md');
  rebaseFixture(dir, { 'CLAUDE.md': base.replace('guide 1', 'pr edit') }, { 'CLAUDE.md': base.replace('guide 1', 'main edit') });
  const s = script(join(dir, 'sub'), ['--continue']);
  assert.equal(s.status, 0, s.stdout + s.stderr);
  assert.match(read(dir, 'CLAUDE.md'), /main edit/);
  assertNoMarkers(read(dir, 'CLAUDE.md'));
  assertClean(dir);
});

// ---------- fail closed ----------

test('paths with no policy stay conflicted and exit 1', () => {
  const dir = newRepo({ 'notes.txt': 'same line\n' });
  const r = rebaseFixture(dir, { 'notes.txt': 'pr line\n' }, { 'notes.txt': 'main line\n' });
  assert.notEqual(r.status, 0);
  assert.deepEqual(unmerged(dir), ['notes.txt']);

  const s = script(dir, []);
  assert.equal(s.status, 1, s.stdout + s.stderr);
  assert.match(s.stdout, /no conflict policy/);
  assert.match(read(dir, 'notes.txt'), /^<{7}/m, 'conflict markers must remain for a human');
  assert.deepEqual(unmerged(dir), ['notes.txt'], 'file must still be unmerged');
});

test('--dry-run reports policies without writing', () => {
  const dir = newRepo({ 'CLAUDE.md': docBase('CLAUDE', 'guide') });
  const base = read(dir, 'CLAUDE.md');
  rebaseFixture(dir, { 'CLAUDE.md': base.replace('guide 1', 'pr edit') }, { 'CLAUDE.md': base.replace('guide 1', 'main edit') });
  const before = read(dir, 'CLAUDE.md');
  const s = script(dir, ['--dry-run']);
  assert.equal(s.status, 0, s.stdout + s.stderr);
  assert.match(s.stdout, /CLAUDE\.md.*doc-ours-plus-incoming/);
  assert.equal(read(dir, 'CLAUDE.md'), before, 'dry-run must not write');
  assert.deepEqual(unmerged(dir), ['CLAUDE.md']);
});

test('no unmerged paths is a clean no-op', () => {
  const dir = newRepo({ 'README.md': 'hi\n' });
  const s = script(dir, []);
  assert.equal(s.status, 0, s.stdout + s.stderr);
  assert.match(s.stdout, /no unmerged paths/);
});

test('--help prints usage', () => {
  const s = script('.', ['--help']);
  assert.equal(s.status, 0, s.stderr);
  assert.match(s.stdout, /--continue/);
});
