#!/usr/bin/env node
// Agent discoverability assets for apps/web/public.
//
// Generated from repo sources of truth:
//
//   version/date     Cargo.toml [workspace.package] version + newest
//                    CHANGELOG.md release (cross-checked with package.json)
//   llms.txt         README.md H1/tagline + docs/*.md H1 titles -> link index
//   llms-full.txt    README + docs heading trees with GitHub links
//   sitemap.xml      every public/*.html page under https://locus.dev
//   robots.txt       allow-all + Sitemap: line
//   capabilities.json  name/version/capabilities/install/commands/tools
//   server.json      MCP registry manifest for io.github.ashlrai/locus-mcp
//   AGENTS.md        copy of the repo AGENTS.md (dash characters normalized)
//   .well-known/security.txt  contact from SECURITY.md, expires release+1y
//   og.png           1200x630 Open Graph card (pure-node PNG, bitmap font)
//
// public/index.html carries paired markers; everything between them is
// generated:
//
//   <!-- disc:KEY --> ...generated... <!-- /disc:KEY -->
//
// Current keys: version (eyebrow label) and json-ld (the SoftwareApplication
// script block). The marker namespace is `disc:`; freshness.mjs owns `fresh:`.
//
// Usage:
//   node apps/web/discoverability.mjs           rewrite all assets in place
//   node apps/web/discoverability.mjs --check   exit 1 when any asset is stale
import {
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  writeFileSync,
} from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { deflateSync } from 'node:zlib';

export const WEB_DIR = dirname(fileURLToPath(import.meta.url));
export const REPO_ROOT = join(WEB_DIR, '..', '..');
export const PUBLIC = join(WEB_DIR, 'public');
export const PAGE = join(PUBLIC, 'index.html');

export const SITE = 'https://locus.dev';
export const REPO = 'https://github.com/ashlrai/locus';
export const REPO_BLOB = `${REPO}/blob/main`;
export const MCP_NAME = 'io.github.ashlrai/locus-mcp';
export const MCP_PACKAGE = '@ashlrai/locus-mcp';
export const NPM_PACKAGE = 'locus-cli';

const read = (root, rel) => readFileSync(join(root, rel), 'utf8');
const kebab = (s) => s.replace(/([a-z0-9])([A-Z])/g, '$1-$2').toLowerCase();
const clean = (s) => s.replace(/[\u2014\u2013]/g, '-').replace(/\s+/g, ' ').trim();
const escXml = (s) =>
  s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;').replace(/'/g, '&apos;');
const escJson = (s) => s.replace(/[\u2018\u2019]/g, "'").replace(/[\u201C\u201D]/g, '"');

// ---------- sources of truth ----------

export function repoVersion(root = REPO_ROOT) {
  const cargo = read(root, 'Cargo.toml').match(
    /\[workspace\.package\][^[]*?\nversion\s*=\s*"([^"]+)"/
  )?.[1];
  if (!cargo) throw new Error('no [workspace.package] version in Cargo.toml');
  const release = read(root, 'CHANGELOG.md').match(
    /^## \[(\d+\.\d+\.\d+)\]\s*[-\u2013\u2014]\s*(\d{4}-\d{2}-\d{2})/m
  );
  if (!release) throw new Error('no released ## [x.y.z] - date heading in CHANGELOG.md');
  if (release[1] !== cargo) {
    throw new Error(`Cargo.toml version ${cargo} != newest CHANGELOG.md release ${release[1]}`);
  }
  const pkg = JSON.parse(read(root, 'apps/web/package.json'));
  if (pkg.version !== cargo) {
    throw new Error(`apps/web/package.json version ${pkg.version} != Cargo workspace ${cargo}`);
  }
  return { version: cargo, date: release[2] };
}

export function mcpTools(root = REPO_ROOT) {
  const src = read(root, 'crates/locus-core/src/adapters/mod.rs');
  const start = src.indexOf('pub fn control_tools(');
  if (start === -1) throw new Error('control_tools() not found in adapters/mod.rs');
  const end = src.indexOf('\n}\n', start);
  const names = [...src.slice(start, end).matchAll(/name:\s*"(locus_[a-z_]+)"/g)].map((m) => m[1]);
  if (!names.length) throw new Error('no locus_* tool names inside control_tools()');
  return [...new Set(names)];
}

// Top-level clap `enum Commands` variants plus their doc-comment first line.
export function cliCommands(root = REPO_ROOT) {
  const src = read(root, 'crates/locus-cli/src/main.rs');
  const start = src.indexOf('enum Commands');
  if (start === -1) throw new Error('enum Commands not found in locus-cli main.rs');
  const brace = src.indexOf('{', start);
  const end = src.indexOf('\n}', brace);
  if (brace === -1 || end === -1) throw new Error('unclosed enum Commands');
  const body = src.slice(brace + 1, end);
  const out = [];
  let doc = '';
  let depth = 0;
  for (const line of body.split('\n')) {
    const d = (line.match(/\{/g) || []).length - (line.match(/\}/g) || []).length;
    if (depth === 0) {
      const docLine = /^\s*\/\/\/\s?(.*)$/.exec(line);
      if (docLine) { doc = docLine[1]; continue; }
      const v = /^    ([A-Z][A-Za-z0-9_]*)\s*[({,]/.exec(line);
      if (v) {
        out.push({ name: `locus ${kebab(v[1])}`, summary: clean(doc) });
        doc = '';
        depth = d;
        continue;
      }
    }
    depth += d;
  }
  if (!out.length) throw new Error('no variants parsed from enum Commands');
  return out;
}

// Install commands, verbatim, from README "## Install options" fenced blocks.
export function installCommands(root = REPO_ROOT) {
  const readme = read(root, 'README.md');
  const secStart = readme.indexOf('## Install options');
  if (secStart === -1) throw new Error('README.md has no "## Install options" section');
  const sec = readme.slice(secStart);
  const secEnd = sec.indexOf('\n## ', 10);
  const body = secEnd === -1 ? sec : sec.slice(0, secEnd);
  const blocks = [...body.matchAll(/```\w*\n([\s\S]*?)```/g)].map((m) => m[1]);
  if (!blocks.length) throw new Error('no fenced blocks under README "## Install options"');
  const cmd = (re) => {
    for (const line of blocks.join('\n').split('\n')) {
      const t = line.trim();
      if (!t || t.startsWith('#')) continue;
      if (re.test(t)) return t;
    }
    throw new Error(`no README install line matching ${re}`);
  };
  return {
    brew: cmd(/^brew install/),
    npm: cmd(/^npm install -g/),
    npx: cmd(/^npx locus-cli/),
    cargo: cmd(/^cargo install --git/),
    local: 'cargo install --path crates/locus-cli && cargo install --path crates/locus-mcp',
  };
}

// README H1 + first descriptive paragraph.
export function readmeInfo(root = REPO_ROOT) {
  const readme = read(root, 'README.md');
  // Disjoint body branches preserve greedy whitespace (including blank lines)
  // without retrying every split of a long whitespace-only heading.
  const title = readme.match(/^#\s+(\S.*|[^\S\r\n\u2028\u2029])$/m)?.[1];
  if (!title) throw new Error('README.md has no H1');
  const tagline = readme.match(/\*\*(Your coding agent[^*]+)\*\*/)?.[1]
    ?? 'AI-native identity plane for coding agents.';
  const h2s = [...readme.matchAll(/^##\s+(\S.*|[^\S\r\n\u2028\u2029])$/gm)].map((m) => clean(m[1]));
  return { title: clean(title), tagline: clean(tagline), sections: h2s };
}

// docs/*.md -> [{ file, title, headings: [{level, text}] }], sorted by file.
export function docsIndex(root = REPO_ROOT) {
  const dir = join(root, 'docs');
  const files = readdirSync(dir).filter((f) => f.toLowerCase().endsWith('.md'))
    .sort((a, b) => a.toLowerCase().localeCompare(b.toLowerCase()));
  if (!files.length) throw new Error('no markdown files under docs/');
  return files.map((file) => {
    const text = read(root, `docs/${file}`);
    const headings = [...text.matchAll(/^(#{1,3})\s+(\S.*|[^\S\r\n\u2028\u2029])$/gm)]
      // Start only at the maximal hash run, avoiding retries at every hash.
      .map((m) => ({ level: m[1].length, text: clean(m[2].replace(/[`[\]*_]/g, '').replace(/(?<!#)#+$/g, '').trim()) }))
      .filter((h) => h.text);
    const title = headings.find((h) => h.level === 1)?.text ?? file.replace(/\.md$/i, '');
    return { file, title, headings: headings.filter((h) => h.level > 1) };
  });
}

export function securityContact(root = REPO_ROOT) {
  const sec = read(root, 'SECURITY.md');
  // A failed local-part run cannot succeed from any of its suffixes.
  const email = sec.match(/(?<![a-z0-9._%+-])[a-z0-9._%+-]+@[a-z0-9.-]+\.[a-z]{2,}/i)?.[0];
  if (!email) throw new Error('SECURITY.md names no contact email');
  return email;
}

// ---------- renderers: every output is a pure function of the inputs ----------

function llmsTxt({ info, docs }) {
  const lines = [
    '# Locus',
    '',
    `> ${info.tagline} AI-native identity plane for coding agents: pin a binding and every CLI command, MCP tool and agent session is sealed to that tenant until a human re-pins. Wrong account, impossible.`,
    '',
    'Identity is resolved at the gate, not in the prompt. Bindings hold principal, tenant, CredentialRefs and policy; sessions are HMAC-sealed to exactly one binding, and agents cannot pin. Sibling product: Phantom (phm.dev) keeps real secret values out of model context.',
    '',
    '## Start',
    '',
    `- [README](${REPO_BLOB}/README.md): install options, 30-second start, MCP wiring`,
    `- [AGENTS.md](${REPO_BLOB}/AGENTS.md): agent operating rules and invariants`,
    `- [capabilities.json](${SITE}/capabilities.json): machine-readable capabilities, install and commands`,
    `- [server.json](${SITE}/server.json): MCP registry manifest for ${MCP_NAME}`,
    '',
    '## Guides',
    '',
    ...docs.map((d) => `- [${d.title}](${REPO_BLOB}/docs/${d.file})`),
    '',
    '## Reference',
    '',
    `- [DESIGN.md](${REPO_BLOB}/DESIGN.md): full architecture and threat model`,
    `- [docs/agent-contract.md](${REPO_BLOB}/docs/agent-contract.md): JSON schemas, exit codes, dry-run`,
    `- [llms-full.txt](${SITE}/llms-full.txt): complete heading index of README and docs`,
    `- [PLAN.md](${REPO_BLOB}/PLAN.md): roadmap`,
    `- [SECURITY.md](${REPO_BLOB}/SECURITY.md): vulnerability reporting`,
    `- [Site](${SITE}/)`,
    '',
  ];
  return lines.join('\n');
}

function llmsFull({ info, docs }) {
  const lines = [
    '# Locus',
    '',
    `> ${info.tagline}`,
    '',
    'Complete heading index generated from README.md and every docs/*.md page.',
    'Fetch the linked Markdown file for full body text; section names are',
    'listed verbatim so an agent can jump straight to the right document.',
    '',
    '## README',
    '',
    `- [README](${REPO_BLOB}/README.md)`,
    ...info.sections.map((s) => `  - ${s}`),
    '',
  ];
  for (const d of docs) {
    lines.push(`## docs/${d.file}`, '', `- [${d.title}](${REPO_BLOB}/docs/${d.file})`);
    for (const h of d.headings) {
      lines.push(`  ${h.level === 3 ? '  ' : ''}- ${h.text}`);
    }
    lines.push('');
  }
  lines.push(
    '## Agent surfaces',
    '',
    `- [capabilities.json](${SITE}/capabilities.json)`,
    `- [server.json](${SITE}/server.json)`,
    `- [robots.txt](${SITE}/robots.txt)`,
    `- [sitemap.xml](${SITE}/sitemap.xml)`,
    `- [AGENTS.md](${SITE}/AGENTS.md)`,
    `- [.well-known/security.txt](${SITE}/.well-known/security.txt)`,
    '',
  );
  return lines.join('\n');
}

function sitemapXml({ pages, date }) {
  const urls = pages.map((p) => {
    const loc = p === 'index.html' ? `${SITE}/` : `${SITE}/${p.replace(/\.html?$/i, '')}`;
    return `  <url>\n    <loc>${escXml(loc)}</loc>\n    <lastmod>${date}</lastmod>\n  </url>`;
  });
  return [
    '<?xml version="1.0" encoding="UTF-8"?>',
    '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">',
    ...urls,
    '</urlset>',
    '',
  ].join('\n');
}

function robotsTxt() {
  return [
    '# Locus - identity plane for coding agents',
    '# Agents: see /llms.txt, /capabilities.json and /server.json',
    'User-agent: *',
    'Allow: /',
    '',
    `Sitemap: ${SITE}/sitemap.xml`,
    '',
  ].join('\n');
}

function capabilitiesJson({ version, install, commands, tools }) {
  return `${escJson(JSON.stringify({
    name: 'Locus',
    version,
    description: 'AI-native identity plane for coding agents. Pin a binding; every CLI command, MCP tool and agent session is sealed to that tenant until a human re-pins.',
    homepage: SITE + '/',
    repository: REPO,
    license: 'MIT',
    capabilities: [
      'pin: seal a session to exactly one binding (human-only; agents may only request)',
      'exec: run child commands with ambient identity scrubbed and only pinned providers injected',
      'mcp: locus-mcp multiplexor exposes control tools plus the pinned binding\'s tools only',
      'freeze: deny model overrides of frozen project_ref, team_id, org and repo scopes',
      'policy: allow/deny/require_approval glob rules with fail-closed defaults',
      'doctor: SAFE/WARN/UNSAFE posture report for the active pin',
      'audit: opt-in signed credential-injection audit log',
      'verify: claim checks (locus_verify_claim) before acting on numbers, URLs or versions',
    ],
    install,
    commands: commands.map((c) => c.name),
    tools,
    mcp: { name: MCP_NAME, manifest: `${SITE}/server.json`, transport: 'stdio' },
    links: {
      llms: `${SITE}/llms.txt`,
      llmsFull: `${SITE}/llms-full.txt`,
      sitemap: `${SITE}/sitemap.xml`,
      agents: `${SITE}/AGENTS.md`,
      docs: REPO_BLOB + '/docs',
    },
  }, null, 2))}\n`;
}

function serverJson({ version }) {
  return `${JSON.stringify({
    $schema: 'https://static.modelcontextprotocol.io/schemas/2025-10-17/server.schema.json',
    name: MCP_NAME,
    description: 'MCP multiplexor for Locus: tools hard-scoped to the active pin so agents cannot act in the wrong tenant.',
    version,
    repository: { url: REPO, source: 'github' },
    packages: [
      {
        registryType: 'npm',
        identifier: MCP_PACKAGE,
        version,
        transport: { type: 'stdio' },
        runtimeHint: 'npx',
      },
    ],
  }, null, 2)}\n`;
}

function securityTxt({ date, email }) {
  const expires = new Date(`${date}T00:00:00Z`);
  expires.setUTCFullYear(expires.getUTCFullYear() + 1);
  return [
    `Contact: mailto:${email}`,
    `Expires: ${expires.toISOString().replace(/\.\d{3}Z$/, '.000Z')}`,
    'Preferred-Languages: en',
    `Canonical: ${SITE}/.well-known/security.txt`,
    `Policy: ${REPO_BLOB}/SECURITY.md`,
    '',
  ].join('\n');
}

// ---------- og.png: pure-node PNG + 5x7 bitmap font ----------

// Classic public-domain 5x7 glyphs for the characters the card uses.
const GLYPHS = {
  ' ': [0, 0, 0, 0, 0, 0, 0],
  '-': [0, 0, 0, 0b11111, 0, 0, 0],
  '.': [0, 0, 0, 0, 0, 0b01100, 0b01100],
  ',': [0, 0, 0, 0, 0b00100, 0b00100, 0b01000],
  '/': [0, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0],
  ':': [0, 0b01100, 0b01100, 0, 0b01100, 0b01100, 0],
  '\u00b7': [0, 0, 0, 0b00100, 0, 0, 0],
  '_': [0, 0, 0, 0, 0, 0, 0b11111],
  '0': [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
  '1': [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
  '2': [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111],
  '3': [0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110],
  '4': [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
  '5': [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
  '6': [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
  '7': [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
  '8': [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
  '9': [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100],
  'A': [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
  'I': [0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
  'M': [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001],
  'T': [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
  'a': [0, 0, 0b01110, 0b00001, 0b01111, 0b10001, 0b01111],
  'b': [0b10000, 0b10000, 0b10110, 0b11001, 0b10001, 0b10001, 0b11110],
  'c': [0, 0, 0b01110, 0b10000, 0b10000, 0b10001, 0b01110],
  'd': [0b00001, 0b00001, 0b01101, 0b10011, 0b10001, 0b10001, 0b01111],
  'e': [0, 0, 0b01110, 0b10001, 0b11111, 0b10000, 0b01110],
  'f': [0b00110, 0b01001, 0b01000, 0b11100, 0b01000, 0b01000, 0b01000],
  'g': [0, 0b01111, 0b10001, 0b10001, 0b01111, 0b00001, 0b01110],
  'h': [0b10000, 0b10000, 0b10110, 0b11001, 0b10001, 0b10001, 0b10001],
  'i': [0b00100, 0, 0b01100, 0b00100, 0b00100, 0b00100, 0b01110],
  'j': [0b00010, 0, 0b00110, 0b00010, 0b00010, 0b10010, 0b01100],
  'k': [0b10000, 0b10000, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010],
  'l': [0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
  'm': [0, 0, 0b11010, 0b10101, 0b10101, 0b10101, 0b10101],
  'n': [0, 0, 0b10110, 0b11001, 0b10001, 0b10001, 0b10001],
  'o': [0, 0, 0b01110, 0b10001, 0b10001, 0b10001, 0b01110],
  'p': [0, 0, 0b11110, 0b10001, 0b11110, 0b10000, 0b10000],
  'q': [0, 0, 0b01101, 0b10011, 0b01111, 0b00001, 0b00001],
  'r': [0, 0, 0b10110, 0b11001, 0b10000, 0b10000, 0b10000],
  's': [0, 0, 0b01111, 0b10000, 0b01110, 0b00001, 0b11110],
  't': [0b01000, 0b01000, 0b11100, 0b01000, 0b01000, 0b01001, 0b00110],
  'u': [0, 0, 0b10001, 0b10001, 0b10001, 0b10011, 0b01101],
  'v': [0, 0, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100],
  'w': [0, 0, 0b10001, 0b10101, 0b10101, 0b10101, 0b01010],
  'x': [0, 0, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001],
  'y': [0, 0, 0b10001, 0b10001, 0b01010, 0b00100, 0b01000],
  'z': [0, 0, 0b11111, 0b00010, 0b00100, 0b01000, 0b11111],
};

const CRC_TABLE = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();

function crc32(buf) {
  let c = 0xffffffff;
  for (const b of buf) c = CRC_TABLE[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function pngChunk(type, data) {
  const out = Buffer.alloc(8 + data.length + 4);
  out.writeUInt32BE(data.length, 0);
  out.write(type, 4, 'ascii');
  data.copy(out, 8);
  out.writeUInt32BE(crc32(out.subarray(4, 8 + data.length)), 8 + data.length);
  return out;
}

export function encodePng(width, height, rgb) {
  const sig = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8; ihdr[9] = 2; // 8-bit RGB
  const raw = Buffer.alloc((width * 3 + 1) * height);
  for (let y = 0; y < height; y++) {
    raw[y * (width * 3 + 1)] = 0; // filter: none
    rgb.copy(raw, y * (width * 3 + 1) + 1, y * width * 3, (y + 1) * width * 3);
  }
  return Buffer.concat([
    sig,
    pngChunk('IHDR', ihdr),
    pngChunk('IDAT', deflateSync(raw, { level: 9 })),
    pngChunk('IEND', Buffer.alloc(0)),
  ]);
}

// Draw a text run; scale is the integer pixel factor for the 5x7 glyphs.
function drawText(px, width, x, y, text, scale, [r, g, b]) {
  for (const ch of text) {
    const glyph = GLYPHS[ch] ?? GLYPHS[ch.toLowerCase()] ?? GLYPHS[' '];
    for (let row = 0; row < 7; row++) {
      for (let col = 0; col < 5; col++) {
        if (!((glyph[row] >> (4 - col)) & 1)) continue;
        for (let dy = 0; dy < scale; dy++) {
          for (let dx = 0; dx < scale; dx++) {
            const i = ((y + row * scale + dy) * width + x + col * scale + dx) * 3;
            px[i] = r; px[i + 1] = g; px[i + 2] = b;
          }
        }
      }
    }
    x += 6 * scale;
  }
}

export function renderOgPng({ version }) {
  const W = 1200, H = 630;
  const px = Buffer.alloc(W * H * 3);
  const put = (x, y, [r, g, b]) => {
    if (x < 0 || y < 0 || x >= W || y >= H) return;
    const i = (y * W + x) * 3;
    px[i] = r; px[i + 1] = g; px[i + 2] = b;
  };
  // background #0a0a0a
  for (let i = 0; i < px.length; i += 3) { px[i] = 10; px[i + 1] = 10; px[i + 2] = 10; }
  // faint grid every 48px, fading toward the bottom like the page mask
  for (let y = 0; y < H; y++) {
    const a = Math.max(0, 1 - y / (H * 0.72)) * 0.05;
    const g = Math.round(10 + 245 * a);
    for (let x = 0; x < W; x += 48) for (let yy = y; yy < Math.min(y + 1, H); yy++) put(x, yy, [g, g, g]);
    for (let x = 0; x < W; x++) if (y % 48 === 0) put(x, y, [g, g, g]);
  }
  // brand row: dot + "locus"
  const cx = 80, cy = 84;
  for (let dy = -9; dy <= 9; dy++) for (let dx = -9; dx <= 9; dx++) {
    const d = Math.hypot(dx, dy);
    if (d <= 5) put(cx + dx, cy + dy, [240, 240, 240]);
    else if (d <= 9) put(cx + dx, cy + dy, [34, 34, 34]);
  }
  drawText(px, W, cx + 26, cy - 21, 'locus', 6, [240, 240, 240]);
  // hero
  drawText(px, W, 78, 236, 'wrong account,', 11, [232, 232, 232]);
  drawText(px, W, 78, 344, 'impossible.', 11, [85, 85, 85]);
  // subline
  drawText(px, W, 80, 496, 'identity plane for coding agents', 4, [138, 138, 138]);
  // footer row
  drawText(px, W, 80, 574, 'github.com/ashlrai/locus', 3, [85, 85, 85]);
  drawText(px, W, 900, 574, `mit - v${version}`, 3, [85, 85, 85]);
  return encodePng(W, H, px);
}

// ---------- index.html generated regions ----------

const OPEN = /<!--\s*disc:([a-z-]+)\s*-->/g;

function jsonLd({ version, info }) {
  const data = {
    '@context': 'https://schema.org',
    '@type': 'SoftwareApplication',
    name: 'Locus',
    applicationCategory: 'DeveloperApplication',
    operatingSystem: ['macOS', 'Linux', 'Windows'],
    softwareVersion: version,
    description: info.tagline,
    url: `${SITE}/`,
    codeRepository: REPO,
    downloadUrl: `${REPO}/releases`,
    license: 'https://opensource.org/licenses/MIT',
    author: { '@type': 'Organization', name: 'AshlrAI', url: 'https://github.com/ashlrai' },
    offers: { '@type': 'Offer', price: '0', priceCurrency: 'USD' },
    screenshot: `${SITE}/og.png`,
  };
  return `\n<script type="application/ld+json">\n${escJson(JSON.stringify(data, null, 2))}\n</script>\n  `;
}

export function buildInputs(root = REPO_ROOT) {
  const { version, date } = repoVersion(root);
  return {
    version,
    date,
    info: readmeInfo(root),
    docs: docsIndex(root),
    install: installCommands(root),
    commands: cliCommands(root),
    tools: mcpTools(root),
    email: securityContact(root),
    pages: readdirSync(PUBLIC).filter((f) => /\.html?$/i.test(f)).sort(),
  };
}

export function buildOutputs(root = REPO_ROOT, inputs = buildInputs(root)) {
  const files = {
    'llms.txt': llmsTxt(inputs),
    'llms-full.txt': llmsFull(inputs),
    'sitemap.xml': sitemapXml(inputs),
    'robots.txt': robotsTxt(),
    'capabilities.json': capabilitiesJson(inputs),
    'server.json': serverJson(inputs),
    '.well-known/security.txt': securityTxt(inputs),
  };
  const binaries = { 'og.png': renderOgPng(inputs) };
  const regions = {
    version: `v${inputs.version}`,
    'json-ld': jsonLd(inputs),
  };
  return { files, binaries, regions };
}

export function applyDisc(html, regions) {
  const missing = Object.keys(regions)
    .filter((k) => !new RegExp(`<!--\\s*disc:${k}\\s*-->`).test(html));
  if (missing.length) throw new Error(`page is missing disc marker(s): ${missing.join(', ')}`);
  let out = '';
  let i = 0;
  let m;
  OPEN.lastIndex = 0;
  while ((m = OPEN.exec(html))) {
    const key = m[1];
    const close = `<!-- /disc:${key} -->`;
    const end = html.indexOf(close, m.index);
    if (end === -1) throw new Error(`unclosed disc marker: ${key}`);
    out += html.slice(i, m.index) + m[0];
    if (!(key in regions)) throw new Error(`unknown disc key: ${key}`);
    out += regions[key];
    i = end;
  }
  return out + html.slice(i);
}

export function run(root = REPO_ROOT, { check = false, out = console.log } = {}) {
  const { files, binaries, regions } = buildOutputs(root);
  const stale = [];
  const write = (rel, contents) => {
    const path = join(PUBLIC, rel);
    const current = existsSync(path) ? readFileSync(path, contents instanceof Buffer ? undefined : 'utf8') : undefined;
    const same = contents instanceof Buffer ? current?.equals(contents) : current === contents;
    if (!same) {
      if (check) { stale.push(rel); return; }
      mkdirSync(dirname(path), { recursive: true });
      writeFileSync(path, contents);
      out(`wrote public/${rel}`);
    }
  };
  for (const [rel, text] of Object.entries(files)) write(rel, text);
  for (const [rel, buf] of Object.entries(binaries)) write(rel, buf);

  // AGENTS.md mirrors the repo file with dash characters normalized, so the
  // whole public tree stays free of em/en dashes.
  const agentsText = read(root, 'AGENTS.md').replace(/[\u2014\u2013]/g, '-');
  const agentsTarget = join(PUBLIC, 'AGENTS.md');
  const agentsSame = existsSync(agentsTarget) && readFileSync(agentsTarget, 'utf8') === agentsText;
  if (!agentsSame) {
    if (check) stale.push('AGENTS.md');
    else { writeFileSync(agentsTarget, agentsText); out('wrote public/AGENTS.md'); }
  }

  // index.html generated regions.
  const html = read(root, 'apps/web/public/index.html');
  const fresh = applyDisc(html, regions);
  if (fresh !== html) {
    if (check) stale.push('index.html (disc regions)');
    else { writeFileSync(PAGE, fresh); out('wrote public/index.html (disc regions)'); }
  }

  if (stale.length) {
    for (const rel of stale) out(`FAIL  stale generated asset: ${rel}`);
    out('      run `node apps/web/discoverability.mjs` to regenerate');
    return 1;
  }
  out('ok    all discoverability assets match the repo sources');
  return 0;
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  try {
    process.exit(run(REPO_ROOT, { check: process.argv.includes('--check') }));
  } catch (e) {
    console.error(`discoverability: ${e.message}`);
    process.exit(1);
  }
}
