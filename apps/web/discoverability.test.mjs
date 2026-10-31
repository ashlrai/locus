// Acceptance tests for the agent-discoverability surface in apps/web/public:
// parses the JSON-LD and llms.txt, validates the sitemap XML, checks every
// page carries exactly one canonical link and one h1, and verifies the
// generator output is in sync with the repo sources of truth.
//
// Run: node --test apps/web/discoverability.test.mjs   (or: npm test)
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, readdirSync, readFileSync, mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import {
  PUBLIC,
  REPO_ROOT,
  SITE,
  REPO,
  MCP_NAME,
  buildInputs,
  buildOutputs,
  applyDisc,
  repoVersion,
} from './discoverability.mjs';

const read = (rel) => readFileSync(join(PUBLIC, rel), 'utf8');
const htmlPages = () =>
  readdirSync(PUBLIC).filter((f) => /\.html?$/i.test(f)).sort();

// ---------- minimal markup helpers (deliberately small, no deps) ----------

function attr(tag, name) {
  const m = new RegExp(`${name}\\s*=\\s*(?:"([^"]*)"|'([^']*)'|([^\\s>]+))`, 'i').exec(tag);
  return m ? (m[1] ?? m[2] ?? m[3]) : undefined;
}

function tags(html, name) {
  return [...html.matchAll(new RegExp(`<${name}\\b[^>]*>`, 'gi'))].map((m) => m[0]);
}

function metaContent(html, key) {
  const lower = key.toLowerCase();
  for (const tag of tags(html, 'meta')) {
    const id = (attr(tag, 'name') ?? attr(tag, 'property') ?? '').toLowerCase();
    if (id === lower) return attr(tag, 'content');
  }
  return undefined;
}

function canonicalHrefs(html) {
  return tags(html, 'link')
    .filter((t) => (attr(t, 'rel') ?? '').toLowerCase().split(/\s+/).includes('canonical'))
    .map((t) => attr(t, 'href'));
}

function headingLevels(html) {
  return [...html.matchAll(/<h([1-6])\b[^>]*>/gi)].map((m) => Number(m[1]));
}

function jsonLdBlocks(html) {
  const re = /<script\b[^>]*type\s*=\s*["']?application\/ld\+json["']?[^>]*>([\s\S]*?)<\/script>/gi;
  return [...html.matchAll(re)].map((m) => m[1]);
}

function markdownLinks(text) {
  return text.match(/\[[^\]]{1,200}\]\([^)\s]{1,500}\)/g) ?? [];
}

// ---------- strict recursive-descent XML parser ----------

function parseXml(xml) {
  let i = 0;
  const fail = (msg) => { throw new Error(`XML parse error at ${i}: ${msg}`); };
  const skipMisc = () => {
    for (;;) {
      const m = /^<\?[\s\S]*?\?>|^<!--[\s\S]*?-->|^<![^>]*>|^\s+/.exec(xml.slice(i));
      if (!m) return;
      i += m[0].length;
    }
  };
  const nameRe = /^[A-Za-z_][\w.:-]*/;
  const text = (stop) => {
    const end = xml.indexOf('<', i);
    const raw = xml.slice(i, end === -1 ? xml.length : end);
    if (end === -1 && !stop) i = xml.length;
    else i = end;
    return raw.replace(/&lt;/g, '<').replace(/&gt;/g, '>')
      .replace(/&quot;/g, '"').replace(/&apos;/g, "'").replace(/&#x?[\da-f]+;/gi, '?')
      .replace(/&amp;/g, '&');
  };
  const element = () => {
    if (xml[i] !== '<') fail('expected element');
    i += 1;
    const name = nameRe.exec(xml.slice(i))?.[0];
    if (!name) fail('bad element name');
    i += name.length;
    const attrs = {};
    for (;;) {
      const ws = /^\s+/.exec(xml.slice(i));
      if (ws) i += ws[0].length;
      if (xml.startsWith('/>', i)) { i += 2; return { name, attrs, children: [], text: '' }; }
      if (xml[i] === '>') { i += 1; break; }
      const a = nameRe.exec(xml.slice(i))?.[0];
      if (!a) fail('bad attribute');
      i += a.length;
      if (xml[i] !== '=') fail('attribute without value');
      i += 1;
      const q = xml[i];
      if (q !== '"' && q !== "'") fail('unquoted attribute');
      const close = xml.indexOf(q, i + 1);
      if (close === -1) fail('unclosed attribute');
      attrs[a] = xml.slice(i + 1, close);
      i = close + 1;
    }
    const node = { name, attrs, children: [], text: '' };
    for (;;) {
      if (i >= xml.length) fail(`unclosed <${name}>`);
      if (xml.startsWith(`</${name}>`, i)) { i += name.length + 3; return node; }
      if (xml.startsWith('</', i)) fail(`mismatched </${xml.slice(i + 2).match(nameRe)?.[0]}>`);
      if (xml.startsWith('<!--', i)) { const e = xml.indexOf('-->', i); if (e === -1) fail('unclosed comment'); i = e + 3; continue; }
      if (xml.startsWith('<![CDATA[', i)) { const e = xml.indexOf(']]>', i); if (e === -1) fail('unclosed CDATA'); node.text += xml.slice(i + 9, e); i = e + 3; continue; }
      if (xml.startsWith('<?', i)) { const e = xml.indexOf('?>', i); if (e === -1) fail('unclosed PI'); i = e + 2; continue; }
      if (xml[i] === '<') { node.children.push(element()); continue; }
      node.text += text();
    }
  };
  skipMisc();
  const root = element();
  skipMisc();
  if (i < xml.length) fail('trailing content after root element');
  return root;
}

// ---------- inputs ----------

const inputs = buildInputs(REPO_ROOT);

// ---------- the acceptance test ----------

test('every page has exactly one canonical link and one h1', () => {
  const pages = htmlPages();
  assert.ok(pages.length >= 1, 'no HTML pages under public/');
  for (const page of pages) {
    const html = read(page);
    const hrefs = canonicalHrefs(html);
    assert.equal(hrefs.length, 1, `${page}: expected exactly one rel=canonical, got ${hrefs.length}`);
    assert.match(hrefs[0], /^https:\/\/locus\.dev\//, `${page}: canonical must be the absolute locus.dev URL`);
    const levels = headingLevels(html);
    assert.equal(levels.filter((l) => l === 1).length, 1, `${page}: expected exactly one h1`);
  }
});

test('every page keeps semantic heading order (no skipped levels)', () => {
  for (const page of htmlPages()) {
    const levels = headingLevels(read(page));
    for (let i = 1; i < levels.length; i++) {
      assert.ok(levels[i] <= levels[i - 1] + 1,
        `${page}: heading jumps h${levels[i - 1]} -> h${levels[i]}`);
    }
  }
});

test('every page carries description, OpenGraph and twitter card meta', () => {
  const required = [
    'description', 'og:title', 'og:description', 'og:type', 'og:url', 'og:image', 'twitter:card',
  ];
  for (const page of htmlPages()) {
    const html = read(page);
    assert.match(html, /<title\b[^>]*>[^<]*\S[^<]*<\/title>/i, `${page}: missing <title>`);
    for (const key of required) {
      assert.ok(metaContent(html, key)?.trim(), `${page}: missing meta ${key}`);
    }
    assert.match(metaContent(html, 'og:image'), /^https:\/\//, 'og:image must be absolute');
  }
});

test('JSON-LD SoftwareApplication parses and matches repo sources', () => {
  const page = read('index.html');
  const blocks = jsonLdBlocks(page);
  assert.equal(blocks.length, 1, `expected one JSON-LD block, got ${blocks.length}`);
  const data = JSON.parse(blocks[0]);
  assert.equal(data['@type'], 'SoftwareApplication');
  assert.equal(data.name, 'Locus');
  assert.equal(data.softwareVersion, repoVersion().publishedVersion, 'JSON-LD must name the verified published release');
  assert.match(String(data.license), /MIT/i, 'JSON-LD license must be MIT');
  assert.equal(data.codeRepository, REPO);
  assert.ok(Array.isArray(data.operatingSystem) && data.operatingSystem.length >= 3,
    'JSON-LD must list supported operating systems');
  assert.equal(data.offers?.['@type'], 'Offer');
  assert.equal(String(data.offers?.price), '0', 'free open-source CLI must advertise price 0');
  assert.equal(data.offers?.priceCurrency, 'USD');
});

test('llms.txt and llms-full.txt have H1, sections and links', () => {
  for (const name of ['llms.txt', 'llms-full.txt']) {
    const text = read(name);
    assert.match(text.slice(0, 4000), /(^|\n)# Locus\n/, `${name}: missing H1`);
    assert.match(text, /(^|\n)## \S/, `${name}: missing a ## section`);
    const links = markdownLinks(text);
    assert.ok(links.length >= 3, `${name}: only ${links.length} markdown link(s)`);
    for (const link of links) {
      assert.match(link, /\]\(https:\/\//, `${name}: link must be absolute https: ${link}`);
    }
  }
  // llms.txt must index every docs page; llms-full must carry the heading trees.
  const llms = read('llms.txt');
  for (const doc of inputs.docs) {
    assert.ok(llms.includes(`docs/${doc.file}`), `llms.txt does not index docs/${doc.file}`);
  }
  const full = read('llms-full.txt');
  for (const section of inputs.info.sections) {
    assert.ok(full.includes(`- ${section}`), `llms-full.txt misses README section "${section}"`);
  }
});

test('sitemap.xml is well-formed XML with absolute https locs per page', () => {
  const root = parseXml(read('sitemap.xml'));
  assert.equal(root.name, 'urlset');
  assert.match(root.attrs.xmlns ?? '', /sitemaps\.org\/schemas\/sitemap/);
  const locs = root.children.filter((c) => c.name === 'url')
    .map((u) => u.children.find((c) => c.name === 'loc')?.text.trim());
  assert.ok(locs.length >= 1, 'sitemap has no <loc> entries');
  const pages = htmlPages();
  assert.equal(locs.length, pages.length, 'sitemap must list every page exactly once');
  for (const loc of locs) {
    assert.match(loc, /^https:\/\/locus\.dev\//, `sitemap loc not on site origin: ${loc}`);
    const rel = loc === `${SITE}/` ? 'index.html' : `${loc.replace(`${SITE}/`, '')}.html`;
    assert.ok(pages.includes(rel), `sitemap loc ${loc} has no page under public/`);
  }
  // each url carries a valid lastmod date
  for (const u of root.children) {
    const lm = u.children.find((c) => c.name === 'lastmod')?.text.trim();
    assert.match(lm, /^\d{4}-\d{2}-\d{2}$/, 'lastmod must be an ISO date');
  }
});

test('XML parser actually rejects malformed input', () => {
  assert.throws(() => parseXml('<a><b></a>'), /XML parse error/);
  assert.throws(() => parseXml('<a>x'), /XML parse error/);
  assert.throws(() => parseXml('not xml'), /XML parse error/);
  assert.throws(() => parseXml('<a/><b/>'), /XML parse error/);
});

test('robots.txt allows crawling and declares the sitemap', () => {
  const robots = read('robots.txt');
  assert.match(robots, /^user-agent:\s*\*/im);
  assert.match(robots, /^allow:\s*\//im);
  const sitemap = robots.match(/^sitemap:\s*(\S+)/im)?.[1];
  assert.equal(sitemap, `${SITE}/sitemap.xml`);
});

test('capabilities.json parses with machine-readable fields', () => {
  const data = JSON.parse(read('capabilities.json'));
  assert.equal(data.name, 'Locus');
  assert.equal(data.version, repoVersion().version);
  assert.ok(data.description?.length > 20);
  for (const key of ['capabilities', 'commands', 'tools']) {
    assert.ok(Array.isArray(data[key]) && data[key].length > 0, `capabilities.json missing ${key}`);
  }
  assert.ok(data.install && typeof data.install === 'object' && Object.keys(data.install).length >= 3,
    'capabilities.json missing install commands');
  assert.equal(data.mcp?.name, MCP_NAME);
});

test('server.json is a valid MCP registry manifest', () => {
  const data = JSON.parse(read('server.json'));
  assert.equal(data.name, MCP_NAME);
  assert.match(data.name, /^[a-z0-9]+(?:[.-][a-z0-9]+)*\/[a-z0-9][a-z0-9._-]*$/i);
  assert.ok(data.description?.trim());
  assert.equal(data.version, repoVersion().publishedVersion);
  assert.equal(data.packages[0].version, repoVersion().publishedVersion);
  assert.ok(Array.isArray(data.packages) && data.packages.length > 0, 'server.json needs packages or remotes');
  assert.equal(data.packages[0].identifier, '@ashlrai/locus-mcp');
});

test('AGENTS.md is served and non-trivial', () => {
  const text = read('AGENTS.md');
  assert.ok(text.length >= 400);
  assert.ok((text.match(/(^|\n)#{1,3}\s+\S/g) ?? []).length >= 2, 'AGENTS.md needs 2+ sections');
  assert.equal(
    text,
    readFileSync(join(REPO_ROOT, 'AGENTS.md'), 'utf8').replace(/[\u2014\u2013]/g, '-'),
    'AGENTS.md must mirror the repo file (dashes normalized)',
  );
});

test('og.png is a real 1200x630 PNG', () => {
  const png = readFileSync(join(PUBLIC, 'og.png'));
  assert.deepEqual([...png.subarray(0, 8)], [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
  assert.equal(png.readUInt32BE(16), 1200, 'IHDR width');
  assert.equal(png.readUInt32BE(20), 630, 'IHDR height');
  assert.equal(png[25], 2, 'color type RGB');
  assert.ok(png.includes('IDAT'), 'missing IDAT chunk');
  assert.ok(png.subarray(-8).includes('IEND'), 'missing IEND chunk');
});

test('generated assets are in sync with repo sources (run generator)', () => {
  const { files, binaries, regions } = buildOutputs(REPO_ROOT, inputs);
  for (const [rel, text] of Object.entries(files)) {
    assert.equal(read(rel), text, `public/${rel} is stale; run node apps/web/discoverability.mjs`);
  }
  for (const [rel, buf] of Object.entries(binaries)) {
    assert.ok(readFileSync(join(PUBLIC, rel)).equals(buf), `public/${rel} is stale`);
  }
  assert.equal(
    readFileSync(join(REPO_ROOT, 'apps/web/public/index.html'), 'utf8'),
    applyDisc(read('index.html'), regions),
    'index.html disc regions are stale',
  );
});

test('no em dashes or en dashes anywhere in public/', () => {
  const files = [];
  const walk = (dir, prefix = '') => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const rel = prefix ? `${prefix}/${entry.name}` : entry.name;
      if (entry.isDirectory()) walk(join(dir, entry.name), rel);
      else if (!/\.png$/i.test(entry.name)) files.push(rel);
    }
  };
  walk(PUBLIC);
  for (const rel of files) {
    const text = read(rel);
    assert.ok(!/[\u2014\u2013]/.test(text), `${rel} contains an em/en dash`);
  }
});

test('security.txt is served under .well-known with required fields', () => {
  const text = read('.well-known/security.txt');
  assert.match(text, /^Contact: mailto:security@ashlr\.ai/m);
  assert.match(text, /^Expires: \d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.000Z/m);
  assert.ok(existsSync(join(PUBLIC, '.well-known', 'security.txt')));
});


test('candidate source version never replaces verified package metadata', () => {
  const candidate = { ...inputs, version: '99.0.0' };
  const output = buildOutputs(REPO_ROOT, candidate);
  const caps = JSON.parse(output.files['capabilities.json']);
  assert.equal(caps.version, '99.0.0');
  assert.match(caps.versionScope, /source checkout/);
  assert.equal(caps.publishedVersion, inputs.publishedVersion);
  const manifest = JSON.parse(output.files['server.json']);
  assert.equal(manifest.version, inputs.publishedVersion);
  assert.equal(manifest.packages[0].version, inputs.publishedVersion);
  assert.equal(output.regions.version, `v${inputs.publishedVersion}`);
  assert.ok(caps.install.npm.includes(`locus-cli@${inputs.publishedVersion}`));
  const rootManifest = JSON.parse(readFileSync(join(REPO_ROOT, 'server.json'), 'utf8'));
  assert.equal(rootManifest.version, inputs.version);
  assert.equal(rootManifest.packages[0].version, inputs.version);
  const sourcePackage = JSON.parse(readFileSync(join(REPO_ROOT, 'npm-mcp/package.json'), 'utf8'));
  assert.equal(rootManifest.packages[0].version, sourcePackage.version);
});


for (const [source, published] of [
  ['0.6.0', '0.5.2'],
  ['0.6.0-rc.1', '0.5.2'],
  ['0.6.0-rc.1', '0.6.0-rc.1'],
  ['0.6.0', '0.6.0-rc.1'],
]) {
  test(`dated source ${source} and published ${published} retain distinct versions`, () => {
    const root = mkdtempSync(join(tmpdir(), 'locus-web-version-'));
    try {
      mkdirSync(join(root, 'apps/web'), { recursive: true });
      mkdirSync(join(root, 'scripts/freshness'), { recursive: true });
      writeFileSync(join(root, 'Cargo.toml'), `[workspace.package]\nversion = "${source}"\n`);
      writeFileSync(join(root, 'apps/web/package.json'), JSON.stringify({ version: source }));
      writeFileSync(join(root, 'scripts/freshness/published.json'), JSON.stringify({ version: published }));
      const headings = `## [${source}] - 2026-10-31\n` +
        (source === published ? '' : `## [${published}] — 2026-10-26\n`);
      writeFileSync(join(root, 'CHANGELOG.md'), headings);
      assert.deepEqual(repoVersion(root), { version: source, date: '2026-10-31', publishedVersion: published });
      writeFileSync(join(root, 'CHANGELOG.md'), headings + `## [${published}] - 2026-10-25\n`);
      assert.throws(() => repoVersion(root), /exactly one dated CHANGELOG entry/);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
}
