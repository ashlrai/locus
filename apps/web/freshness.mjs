#!/usr/bin/env node
// Site freshness for apps/web/public/index.html.
//
// Hand-maintained version strings, install commands, the CLI feature list and
// the MCP tool list are generated from the repo sources of truth:
//
//   version        Cargo.toml [workspace.package] version (cross-checked
//                  against the newest CHANGELOG.md release and package.json)
//   install tabs   README.md "## Install options" fenced blocks
//   CLI catalog    clap `enum Commands` plus the *Cmd subcommand enums in
//                  crates/locus-cli/src/main.rs (what `locus --help` prints)
//   MCP tools      control_tools() in crates/locus-core/src/adapters/mod.rs
//
// The page carries paired markers; everything between them is generated:
//
//   <!-- fresh:KEY --> ...generated... <!-- /fresh:KEY -->
//
// Usage:
//   node apps/web/freshness.mjs           rewrite marked regions in place
//   node apps/web/freshness.mjs --check   exit 1 if the page is stale or a
//                                       hand-written command/version is wrong
//
// Beyond the marked regions, --check also scans the remaining page text:
// every `locus <sub>` must be a real CLI command (depth 2 against the
// subcommand enums), every `locus_*` identifier must be a real MCP control
// tool, and every version literal must equal the Cargo workspace version.
import { readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

export const WEB_DIR = dirname(fileURLToPath(import.meta.url));
export const REPO_ROOT = join(WEB_DIR, '..', '..');
export const PAGE = join(WEB_DIR, 'public', 'index.html');

const read = (root, rel) => readFileSync(join(root, rel), 'utf8');
const kebab = (s) => s.replace(/([a-z0-9])([A-Z])/g, '$1-$2').toLowerCase();
const clean = (s) => s.replace(/[\u2014\u2013]/g, '-').replace(/\s+/g, ' ').trim();
const esc = (s) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');

// ---------- sources of truth ----------

export function repoVersion(root = REPO_ROOT) {
  const cargo = read(root, 'Cargo.toml').match(
    /\[workspace\.package\][^[]*?\nversion\s*=\s*"([^"]+)"/
  )?.[1];
  if (!cargo) throw new Error('no [workspace.package] version in Cargo.toml');
  const changelog = read(root, 'CHANGELOG.md').match(/^## \[(\d+\.\d+\.\d+)\]/m)?.[1];
  if (!changelog) throw new Error('no released ## [x.y.z] heading in CHANGELOG.md');
  if (changelog !== cargo) {
    throw new Error(`Cargo.toml version ${cargo} != newest CHANGELOG.md release ${changelog}`);
  }
  const pkg = JSON.parse(read(root, 'apps/web/package.json'));
  if (pkg.version !== cargo) {
    throw new Error(`apps/web/package.json version ${pkg.version} != Cargo workspace ${cargo}`);
  }
  return cargo;
}

// Parse every top-level `enum Name { ... }` in a Rust file into
// Map<enumName, [{ name, group, subs }]>. `subs` is the list of other parsed
// enums the variant payload references - but only when clap's `subcommand`
// attribute is present, so plain field enums (e.g. a `value_enum` option type
// like CliEventsExportSink) are not mistaken for subcommands.
export function parseEnums(src) {
  const bodies = new Map();
  const enumRe = /^enum\s+(\w+)\s*\{/gm;
  let m;
  while ((m = enumRe.exec(src))) {
    const end = src.indexOf('\n}', enumRe.lastIndex);
    if (end === -1) throw new Error(`unclosed enum ${m[1]}`);
    bodies.set(m[1], src.slice(enumRe.lastIndex, end));
  }
  const out = new Map();
  for (const [name, body] of bodies) {
    const variants = [];
    let pending = '';
    let cur = null;
    let depth = 0;
    const delta = (line) => (line.match(/\{/g) || []).length - (line.match(/\}/g) || []).length;
    for (const line of body.split('\n')) {
      const v = depth === 0 && /^    ([A-Z][A-Za-z0-9_]*)\s*[({,]?/.exec(line);
      if (v) {
        const heads = [...pending.matchAll(/next_help_heading\s*=\s*"([^"]+)"/g)];
        cur = { name: v[1], group: heads.at(-1)?.[1] ?? null, attrs: pending, body: line };
        variants.push(cur);
        pending = '';
        depth = delta(line);
        continue;
      }
      if (cur && depth > 0) {
        cur.body += '\n' + line;
        depth += delta(line);
        continue;
      }
      pending += line + '\n';
    }
    for (const v of variants) {
      v.subs = /\bsubcommand\b/.test(v.attrs + v.body)
        ? [...v.body.matchAll(/\b([A-Z]\w*)\b/g)]
            .map((t) => t[1])
            .filter((t) => t !== v.name && bodies.has(t))
        : [];
    }
    out.set(name, variants);
  }
  return out;
}

// The CLI surface: ordered help-heading groups, each with the top-level
// commands and each command's own subcommands.
export function cliSurface(root = REPO_ROOT) {
  const enums = parseEnums(read(root, 'crates/locus-cli/src/main.rs'));
  const top = enums.get('Commands');
  if (!top) throw new Error('enum Commands not found in locus-cli main.rs');
  const groups = [];
  const byName = new Map();
  const subsOf = (variants) =>
    (variants || []).map((v) => kebab(v.name) + (v.subs.length ? ` [${subsOf(enums.get(v.subs[0])).join(' ')}]` : ''));
  for (const v of top) {
    const group = v.group ?? 'Other';
    if (!byName.has(group)) {
      byName.set(group, []);
      groups.push(group);
    }
    const entry = { name: kebab(v.name), group, subs: v.subs[0] ? subsOf(enums.get(v.subs[0])) : [] };
    byName.get(group).push(entry);
  }
  // resolve a command path: tokens[0] a top-level name, deeper tokens matched
  // against the referenced subcommand enums. Returns the error string or null.
  const flatSub = (variants) => (variants || []).map((v) => kebab(v.name));
  const checkPath = (tokens) => {
    let variants = top;
    for (let i = 0; i < tokens.length; i++) {
      const v = variants.find((x) => kebab(x.name) === tokens[i]);
      if (!v) {
        return `\`locus ${tokens.slice(0, i + 1).join(' ')}\`: unknown ${i === 0 ? 'command' : 'subcommand'} ` +
          `"${tokens[i]}" (expected one of: ${flatSub(variants).join(', ')})`;
      }
      variants = v.subs[0] ? enums.get(v.subs[0]) : null;
      if (!variants && i + 1 < tokens.length) return null; // deeper words are args
    }
    return null;
  };
  const topLevel = () => flatSub(top);
  return { groups, byName, checkPath, topLevel };
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

// Fenced blocks of the README "## Install options" section: block[0] holds the
// brew/npm/cargo segments (blank-line separated), the "Local checkout" block
// follows as its own fence.
export function installBlocks(root = REPO_ROOT) {
  const readme = read(root, 'README.md');
  const secStart = readme.indexOf('## Install options');
  if (secStart === -1) throw new Error('README.md has no "## Install options" section');
  const sec = readme.slice(secStart);
  const secEnd = sec.indexOf('\n## ', 10);
  const body = secEnd === -1 ? sec : sec.slice(0, secEnd);
  const blocks = [...body.matchAll(/```\w*\n([\s\S]*?)```/g)].map((m) => m[1].replace(/\n$/, ''));
  if (!blocks.length) throw new Error('no fenced blocks under README "## Install options"');
  return blocks;
}

function segItems(seg, comments) {
  let firstCmt = true;
  const items = [];
  for (const raw of seg.split('\n')) {
    const line = raw.trimEnd();
    if (!line.trim()) continue;
    const cmt = line.match(/^\s*#\s?(.*)$/);
    if (cmt) {
      if (comments === 'all' || (comments === 'first' && firstCmt)) items.push({ cmt: clean(cmt[1]) });
      firstCmt = false;
      continue;
    }
    const trail = line.match(/^(.*?)\s{2,}#\s?(.*)$/);
    if (trail) items.push({ cmd: trail[1].trim(), cmt: clean(trail[2]) });
    else items.push({ cmd: line.trim() });
  }
  return items;
}

// items -> <span> lines, deduping repeated command lines within the tab.
function renderItems(items) {
  const seen = new Set();
  const out = [];
  for (const it of items) {
    if (it.cmd) {
      if (seen.has(it.cmd)) continue;
      seen.add(it.cmd);
      out.push(`<span class="cmd">${esc(it.cmd)}</span>` + (it.cmt ? `  <span class="cmt"># ${esc(it.cmt)}</span>` : ''));
    } else if (it.cmt) {
      out.push(`<span class="cmt"># ${esc(it.cmt)}</span>`);
    }
  }
  return out;
}

const segmentWith = (blocks, re) => {
  const seg = blocks[0].split(/\n\s*\n/).find((s) => re.test(s));
  if (!seg) throw new Error(`README install block has no segment matching ${re}`);
  return seg;
};

// Published binary names (locus, locus-cli, locus-mcp) the page may mention.
export function crateNames(root = REPO_ROOT) {
  const names = new Set();
  for (const dir of ['crates/locus-core', 'crates/locus-cli', 'crates/locus-mcp']) {
    const name = read(root, `${dir}/Cargo.toml`).match(/^name\s*=\s*"([^"]+)"/m)?.[1];
    if (name) names.add(name);
  }
  return names;
}

// ---------- generated regions ----------

export function buildRegions(root = REPO_ROOT) {
  const version = repoVersion(root);
  const tools = mcpTools(root);
  const cli = cliSurface(root);
  const blocks = installBlocks(root);
  const local = blocks.find((b) => b.includes('cargo install --path'));

  const install = (re, comments, extra = []) => {
    const items = [...segItems(segmentWith(blocks, re), comments), ...extra];
    return renderItems(items).join('\n');
  };

  const catalog = cli.groups.map((g) => {
    const cmds = cli.byName.get(g).map((c) => c.name + (c.subs.length ? ` [${c.subs.join(' ')}]` : ''));
    return `<div class="catline"><span class="g">${esc(g.toLowerCase())}</span><span>${esc(cmds.join(' \u00b7 '))}</span></div>`;
  });

  const regions = {
    version: `v${version}`,
    'install-brew': install(/brew\s+install/, 'all'),
    'install-npm': install(/npm\s+install/, 'all'),
    'install-cargo': install(/cargo\s+install\s+--git/, 'first', local
      ? [{ cmt: 'or from a local checkout' }, ...segItems(local, 'none')]
      : []),
    'mcp-tools': 'control tools: ' + tools.map((t) => `<code>${t}</code>`).join(' '),
    'cli-catalog': catalog.join('\n'),
  };
  return { regions, cli, tools, version, crates: crateNames(root) };
}

// ---------- page rewrite + page-wide checks ----------

const OPEN = /<!--\s*fresh:([a-z-]+)\s*-->/g;

export function applyFreshness(html, regions) {
  const missing = Object.keys(regions)
    .filter((k) => typeof regions[k] === 'string')
    .filter((k) => !new RegExp(`<!--\\s*fresh:${k}\\s*-->`).test(html));
  if (missing.length) throw new Error(`page is missing freshness marker(s): ${missing.join(', ')}`);
  let out = '';
  let i = 0;
  let m;
  OPEN.lastIndex = 0;
  while ((m = OPEN.exec(html))) {
    const key = m[1];
    const close = `<!-- /fresh:${key} -->`;
    const end = html.indexOf(close, m.index);
    if (end === -1) throw new Error(`unclosed freshness marker: ${key}`);
    out += html.slice(i, m.index) + m[0];
    if (!(key in regions)) throw new Error(`unknown freshness key: ${key}`);
    out += regions[key];
    i = end;
  }
  return out + html.slice(i);
}

// Strip style/script blocks and marked regions, then scan the remaining text.
export function validateHtml(html, { cli, tools, version, crates = new Set() }) {
  const problems = [];
  let text = html.replace(/<style>[\s\S]*?<\/style>/g, ' ').replace(/<script>[\s\S]*?<\/script>/g, ' ');
  text = text.replace(/<!--\s*fresh:[a-z-]+\s*-->[\s\S]*?<!--\s*\/fresh:[a-z-]+\s*-->/g, ' ');

  for (const m of text.matchAll(/\blocus\s+([a-z][a-z0-9-]*)(?:\s+([a-z][a-z0-9-]*))?/g)) {
    const tokens = [m[1]];
    if (m[2]) tokens.push(m[2]);
    const err = cli.checkPath(tokens);
    if (err) problems.push(err);
  }
  const toolSet = new Set(tools);
  for (const m of text.matchAll(/\blocus_[a-z_]+\b/g)) {
    if (!toolSet.has(m[0])) {
      problems.push(`\`${m[0]}\` is not an MCP control tool (see control_tools() in locus-core)`);
    }
  }
  for (const m of text.matchAll(/\blocus-[a-z]+\b/g)) {
    if (crates.size && !crates.has(m[0])) {
      problems.push(`\`${m[0]}\` is not a workspace crate (expected: ${[...crates].join(', ')})`);
    }
  }
  for (const m of text.matchAll(/\bv(\d+\.\d+(?:\.\d+)?)\b|(?<![\d.])(\d+\.\d+\.\d+)\b(?!\.\d)/g)) {
    const found = m[1] ?? m[2];
    if (found !== version) {
      problems.push(`version literal "${m[0]}" does not match Cargo workspace version ${version}`);
    }
  }
  return [...new Set(problems)];
}

export function run(root = REPO_ROOT, { check = false, out = console.log } = {}) {
  const data = buildRegions(root);
  const html = read(root, 'apps/web/public/index.html');
  const fresh = applyFreshness(html, data.regions);
  let failed = false;

  if (check) {
    if (fresh !== html) {
      failed = true;
      const keys = [...html.matchAll(/<!--\s*fresh:([a-z-]+)\s*-->/g)].map((m) => m[1]);
      const stale = keys.filter((k, i) => keys.indexOf(k) === i).filter((k) => {
        const re = new RegExp(`<!--\\s*fresh:${k}\\s*-->([\\s\\S]*?)<!--\\s*\\/fresh:${k}\\s*-->`);
        return html.match(re)[1] !== fresh.match(re)[1];
      });
      out(`FAIL  stale generated region(s): ${[...new Set(stale)].join(', ')}`);
      out('      run `node apps/web/freshness.mjs` to regenerate');
    } else {
      out('ok    generated regions match the repo sources');
    }
  }

  const problems = validateHtml(fresh, data);
  for (const p of problems) out(`FAIL  ${p}`);
  if (!problems.length) out('ok    every `locus` command, `locus_*` tool and version literal is current');

  if (!check && fresh !== html) {
    writeFileSync(join(root, 'apps/web/public/index.html'), fresh);
    out('wrote apps/web/public/index.html');
  }
  return failed || problems.length ? 1 : 0;
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  try {
    process.exit(run(REPO_ROOT, { check: process.argv.includes('--check') }));
  } catch (e) {
    console.error(`freshness: ${e.message}`);
    process.exit(1);
  }
}
