#!/usr/bin/env node
// Acceptance tests for the built site in public/.
// Run: node --test scripts/test-site.mjs
import { execSync } from "node:child_process";
import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { SITE_ORIGIN, REPO_URL, REPO_ROOT, WEB_ROOT } from "../site.config.mjs";

const PUBLIC = path.join(WEB_ROOT, "public");
const SRC_PAGES = path.join(WEB_ROOT, "src", "pages");
const TODAY = new Date().toISOString().slice(0, 10);

function* walk(dir, base = dir) {
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) yield* walk(p, base);
    else yield path.relative(base, p);
  }
}

const pages = [...walk(PUBLIC)].filter((f) => f.endsWith(".html"));
const read = (rel) => fs.readFileSync(path.join(PUBLIC, rel), "utf8");

const canonicalPath = (rel) => {
  const p = rel.replace(/\.html$/, "");
  return p === "index" ? "/" : `/${p}`;
};

// Extract inner HTML of elements carrying `data-claim` (balanced tag scan).
function claimBlocks(html) {
  const blocks = [];
  const re = /<(li|div|article|section|p)\b[^>]*\bdata-claim\b[^>]*>/g;
  let m;
  while ((m = re.exec(html))) {
    const tag = m[1];
    const open = new RegExp(`<${tag}\\b`, "g");
    const close = new RegExp(`</${tag}>`, "g");
    let depth = 1;
    let i = m.index + m[0].length;
    while (depth > 0 && i < html.length) {
      open.lastIndex = close.lastIndex = i;
      const o = open.exec(html);
      const c = close.exec(html);
      if (!c) break;
      if (o && o.index < c.index) {
        depth++;
        i = o.index + 1;
      } else {
        depth--;
        if (depth === 0) {
          blocks.push(html.slice(m.index + m[0].length, c.index));
          break;
        }
        i = c.index + 1;
      }
    }
  }
  return blocks;
}

const textOf = (html) => html.replace(/<[^>]+>/g, " ");
const srcLinks = (block) =>
  [...block.matchAll(/<a\b[^>]*class="src[^"]*"[^>]*href="([^"]+)"[^>]*>/g)].map((m) => m[1]);

function repoPathExists(url) {
  const m = url.match(/^https:\/\/github\.com\/ashlrai\/locus\/(blob|tree)\/main\/([^#]+)/);
  if (!m) return null; // not a repo-main link
  const abs = path.join(REPO_ROOT, decodeURIComponent(m[2]));
  return fs.existsSync(abs);
}

function gitLastmod(srcRel) {
  try {
    const out = execSync(
      `git -C ${JSON.stringify(REPO_ROOT)} log -1 --format=%cs -- ${JSON.stringify(srcRel)}`,
      { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }
    ).trim();
    return out || null;
  } catch {
    return null;
  }
}

test("site builds and public/ is in sync", () => {
  const r = execSync(`node ${JSON.stringify(path.join(WEB_ROOT, "scripts", "build.mjs"))} --check`, {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  });
  assert.match(r, /in sync/);
});

test("every page has exactly one absolute canonical on the site origin", () => {
  assert.ok(pages.length >= 5, `expected >=5 pages, found ${pages.length}`);
  for (const rel of pages) {
    const html = read(rel);
    const links = [...html.matchAll(/<link\b[^>]*rel="canonical"[^>]*href="([^"]+)"/g)];
    assert.equal(links.length, 1, `${rel}: expected 1 canonical, found ${links.length}`);
    const want = SITE_ORIGIN + canonicalPath(rel);
    assert.equal(links[0][1], want, `${rel}: canonical ${links[0][1]} != ${want}`);
    assert.ok(!links[0][1].endsWith(".html"), `${rel}: canonical must be extensionless`);
  }
});

test("every page has title, meta description, one h1, lang", () => {
  for (const rel of pages) {
    const html = read(rel);
    assert.match(html, /<title>[^<]{5,}<\/title>/, `${rel}: title`);
    const d = html.match(/<meta name="description" content="([^"]+)"/);
    assert.ok(d, `${rel}: meta description`);
    assert.ok(d[1].length >= 50 && d[1].length <= 165, `${rel}: description ${d[1].length} chars`);
    assert.equal([...html.matchAll(/<h1\b/g)].length, 1, `${rel}: exactly one h1`);
    assert.match(html, /<html lang="en">/, `${rel}: lang`);
    const og = html.match(/<meta property="og:url" content="([^"]+)"/);
    assert.ok(og && og[1] === SITE_ORIGIN + canonicalPath(rel), `${rel}: og:url == canonical`);
  }
});

test("every claim block carries a source link that resolves", () => {
  let total = 0;
  for (const rel of pages) {
    for (const block of claimBlocks(read(rel))) {
      total++;
      const srcs = srcLinks(block);
      assert.ok(srcs.length >= 1, `${rel}: claim without source link:\n${textOf(block).trim().slice(0, 120)}`);
      for (const href of srcs) {
        assert.match(href, /^https:\/\//, `${rel}: source must be https, got ${href}`);
        assert.ok(!href.startsWith(SITE_ORIGIN), `${rel}: self-citation is not a source: ${href}`);
        const exists = repoPathExists(href);
        if (exists !== null) {
          assert.ok(exists, `${rel}: cited repo path does not exist: ${href}`);
        }
      }
      // A claim containing numbers must trace to a repo file, not only an external site.
      if (/\d/.test(textOf(block))) {
        assert.ok(
          srcs.some((h) => repoPathExists(h)),
          `${rel}: numeric claim needs a repo source:\n${textOf(block).trim().slice(0, 120)}`
        );
      }
    }
  }
  assert.ok(total >= 20, `expected >=20 claim blocks, found ${total}`);
});

test("sitemap is search-console-ready", () => {
  const xml = read("sitemap.xml");
  assert.match(xml, /<urlset xmlns="http:\/\/www\.sitemaps\.org\/schemas\/sitemap\/0\.9">/);
  const entries = [...xml.matchAll(/<url><loc>([^<]+)<\/loc><lastmod>([^<]+)<\/lastmod><\/url>/g)];
  const locs = entries.map((e) => e[1]);
  const want = pages.map((p) => SITE_ORIGIN + canonicalPath(p)).sort();
  assert.deepEqual([...locs].sort(), want, "sitemap URLs must cover every page and nothing else");
  for (const [, loc, lastmod] of entries) {
    assert.match(loc, new RegExp(`^${SITE_ORIGIN.replace(/[/.]/g, "\\$&")}(/|$)`), `absolute loc: ${loc}`);
    assert.match(lastmod, /^\d{4}-\d{2}-\d{2}$/, `lastmod format: ${lastmod}`);
    assert.ok(lastmod <= TODAY, `lastmod in the future: ${lastmod}`);
  }
  // For pages whose source is committed, lastmod must equal the git commit date.
  for (const rel of pages) {
    const srcRel = path.relative(REPO_ROOT, path.join(SRC_PAGES, rel));
    const committed = gitLastmod(srcRel);
    if (committed) {
      const e = entries.find((x) => x[1] === SITE_ORIGIN + canonicalPath(rel));
      assert.equal(e[2], committed, `${rel}: sitemap lastmod != git lastmod`);
    }
  }
});

test("robots.txt points at the sitemap", () => {
  const robots = read("robots.txt");
  assert.match(robots, new RegExp(`Sitemap: ${SITE_ORIGIN.replace(/[/.]/g, "\\$&")}/sitemap\\.xml`));
  assert.match(robots, /User-agent: \*/);
});

test("internal links resolve", () => {
  for (const rel of pages) {
    const html = read(rel);
    for (const m of html.matchAll(/href="(\/[^"#?]*)[^"#?]*"/g)) {
      const p = m[1];
      const f = p === "/" ? "index.html" : `${p.slice(1)}.html`;
      assert.ok(
        fs.existsSync(path.join(PUBLIC, f)) || fs.existsSync(path.join(PUBLIC, p.slice(1))),
        `${rel}: internal link ${p} resolves to nothing`
      );
    }
  }
});

test("no em or en dashes in page markup", () => {
  for (const rel of pages) {
    const html = read(rel);
    assert.ok(!/[‐‑‒–—―]/.test(html), `${rel}: contains an en/em dash`);
    assert.ok(!/&(mdash|ndash);|&#x201[34];|&#821[12];/.test(html), `${rel}: dash entity`);
  }
});

test("json-ld blocks parse", () => {
  for (const rel of pages) {
    for (const m of read(rel).matchAll(/<script type="application\/ld\+json">([\s\S]*?)<\/script>/g)) {
      const doc = JSON.parse(m[1]);
      assert.equal(doc["@context"], "https://schema.org", `${rel}: json-ld context`);
    }
  }
});

test("cwv guards: no unsized images, no external fonts or render-blocking assets", () => {
  for (const rel of pages) {
    const html = read(rel);
    for (const m of html.matchAll(/<img\b[^>]*>/g)) {
      assert.match(m[0], /\bwidth=/, `${rel}: <img> missing width`);
      assert.match(m[0], /\bheight=/, `${rel}: <img> missing height`);
    }
    assert.ok(!/<link\b[^>]*rel="stylesheet"/.test(html), `${rel}: external stylesheet (CSS must be inlined)`);
    assert.ok(!/fonts\.(googleapis|gstatic)\.com/.test(html), `${rel}: external font request`);
    assert.ok(!/@font-face/.test(html), `${rel}: webfont without font-display discipline`);
    assert.ok(!/<script\b(?![^>]*\b(defer|async)\b)[^>]*\bsrc=/.test(html), `${rel}: render-blocking external script`);
  }
});

test("no secret-shaped strings in pages", () => {
  for (const rel of pages) {
    const html = read(rel);
    assert.ok(
      !/\b(?:ghp_|gho_|github_pat_|sk-[A-Za-z0-9]|AKIA[0-9A-Z]|phm_)[A-Za-z0-9_]{6,}/.test(html),
      `${rel}: possible secret literal`
    );
  }
});
