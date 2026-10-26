import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import {
  WEB_DIR, REPO_ROOT,
  repoVersion, cliSurface, mcpTools, installBlocks,
  buildRegions, applyFreshness, validateHtml, run,
} from './freshness.mjs';

const page = readFileSync(join(WEB_DIR, 'public', 'index.html'), 'utf8');

test('repoVersion: Cargo workspace == newest CHANGELOG release == package.json', () => {
  const v = repoVersion();
  assert.match(v, /^\d+\.\d+\.\d+$/);
  const cargo = readFileSync(join(REPO_ROOT, 'Cargo.toml'), 'utf8');
  assert.ok(cargo.includes(`version = "${v}"`));
});

test('cliSurface: groups, top-level commands and nested subcommands', () => {
  const cli = cliSurface();
  for (const cmd of ['enter', 'leave', 'whoami', 'exec', 'doctor', 'dashboard', 'ci', 'forensics', 'verify']) {
    assert.ok(cli.topLevel().includes(cmd), `missing top-level command ${cmd}`);
  }
  assert.equal(cli.checkPath(['enter', 'acme']), null);          // positional arg is fine
  assert.equal(cli.checkPath(['verify', 'claim']), null);
  assert.equal(cli.checkPath(['ci', 'run']), null);
  assert.equal(cli.checkPath(['agent', 'report']), null);
  assert.match(cli.checkPath(['bogus']), /unknown command "bogus"/);
  assert.match(cli.checkPath(['verify', 'bogus']), /unknown subcommand "bogus"/);
});

test('mcpTools: the control_tools() catalog', () => {
  const tools = mcpTools();
  for (const t of ['locus_whoami', 'locus_safe_next', 'locus_request_pin', 'locus_verify_claim', 'locus_verify_session']) {
    assert.ok(tools.includes(t), `missing ${t}`);
  }
  assert.equal(new Set(tools).size, tools.length, 'duplicate tool names');
});

test('installBlocks: brew, npm, cargo and local-checkout commands', () => {
  const blocks = installBlocks();
  const all = blocks.join('\n');
  for (const cmd of [
    'brew install ashlrai/tap/locus',
    'npm install -g locus-cli @ashlrai/locus-mcp',
    'npx locus-cli --help',
    'npx @ashlrai/locus-mcp',
    'cargo install --git https://github.com/ashlrai/locus locus-cli locus-mcp --locked',
    'cargo install --path crates/locus-cli',
  ]) {
    assert.ok(all.includes(cmd), `README install section lost "${cmd}"`);
  }
});

test('applyFreshness: regenerates marked regions and is idempotent', () => {
  const { regions } = buildRegions();
  const once = applyFreshness(page, regions);
  assert.equal(applyFreshness(once, regions), once);
  assert.match(once, /<!--\s*fresh:version\s*-->v\d+\.\d+\.\d+<!--\s*\/fresh:version\s*-->/);
  assert.ok(once.includes(`npx @ashlrai/locus-mcp`), 'npm tab should come from the README');
  assert.ok(once.includes('<code>locus_whoami</code>'), 'MCP tool list should be rendered');
  assert.ok(once.includes('ci [mint env run]'), 'CLI catalog should render subcommands');
});

test('applyFreshness: rejects unknown keys and missing markers', () => {
  const { regions } = buildRegions();
  assert.throws(
    () => applyFreshness(page + '<!-- fresh:bogus -->x<!-- /fresh:bogus -->', regions),
    /unknown freshness key: bogus/
  );
  assert.throws(() => applyFreshness('<p>no markers</p>', regions), /missing freshness marker/);
  assert.throws(
    () => applyFreshness(page + '<!-- fresh:version -->v0', regions),
    /unclosed freshness marker: version/
  );
});

test('validateHtml: flags dead commands, dead tools and stale versions', () => {
  const data = buildRegions();
  const { cli, tools, version } = data;
  const okPage = `locus doctor\nlocus verify claim\nlocus_whoami\nv${version}`;
  assert.deepEqual(validateHtml(okPage, { cli, tools, version }), []);
  const bad = validateHtml('run `locus frobnicate` and `locus_nosuch` since v9.9.9', { cli, tools, version });
  assert.ok(bad.some((p) => p.includes('unknown command "frobnicate"')), bad.join('\n'));
  assert.ok(bad.some((p) => p.includes('locus_nosuch')), bad.join('\n'));
  assert.ok(bad.some((p) => p.includes('v9.9.9')), bad.join('\n'));
});

test('validateHtml: subcommand typos fail; positional args pass', () => {
  const { cli, tools, version } = buildRegions();
  const bad = validateHtml('locus verify cliam', { cli, tools, version });
  assert.ok(bad.some((p) => p.includes('unknown subcommand "cliam"')));
  assert.deepEqual(validateHtml('locus enter acme && locus whoami', { cli, tools, version }), []);
});

test('validateHtml: binary names must be workspace crates', () => {
  const { cli, tools, version, crates } = buildRegions();
  assert.ok(crates.has('locus-cli') && crates.has('locus-mcp'));
  assert.deepEqual(validateHtml('locus-mcp --http && npx locus-cli --help', { cli, tools, version, crates }), []);
  const bogus = validateHtml('locus-frobnicator', { cli, tools, version, crates });
  assert.ok(bogus.some((p) => p.includes('`locus-frobnicator` is not a workspace crate')), bogus.join('\n'));
});

test('run --check: passes on the committed page and fails on staleness', () => {
  const logs = [];
  const out = (s) => logs.push(s);
  assert.equal(run(REPO_ROOT, { check: true, out }), 0, logs.join('\n'));

  const stale = page.replace(/(<!--\s*fresh:version\s*-->)[^<]*<!--\s*\/fresh:version\s*-->/, '$1v9.9.9<!-- /fresh:version -->');
  const { regions } = buildRegions();
  assert.notEqual(applyFreshness(stale, regions), stale, 'a hand-edited version must differ after regeneration');
  assert.equal(applyFreshness(page, regions), page, 'committed page is not fresh - run node apps/web/freshness.mjs');
});
