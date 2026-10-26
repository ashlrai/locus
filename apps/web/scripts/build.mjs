#!/usr/bin/env node
// Builds apps/web/public from apps/web/src.
//   node scripts/build.mjs          write public/
//   node scripts/build.mjs --check  verify public/ matches sources (no writes)
import { execSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { SITE_ORIGIN, SITE_NAME, REPO_URL, REPO_BLOB, REPO_ROOT, WEB_ROOT } from "../site.config.mjs";

const SRC = path.join(WEB_ROOT, "src");
const PAGES = path.join(SRC, "pages");
const PUBLIC = path.join(WEB_ROOT, "public");
const CHECK = process.argv.includes("--check");

const esc = (s) =>
  String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");

function parsePage(file) {
  const raw = fs.readFileSync(file, "utf8");
  const m = raw.match(/^---\n([\s\S]*?)\n---\n?([\s\S]*)$/);
  if (!m) throw new Error(`missing front-matter: ${file}`);
  const meta = {};
  for (const line of m[1].split("\n")) {
    const kv = line.match(/^([a-z_]+):\s*(.*)$/);
    if (kv) meta[kv[1]] = kv[2].trim();
  }
  for (const req of ["title", "description"]) {
    if (!meta[req]) throw new Error(`front-matter missing "${req}": ${file}`);
  }
  return { meta, body: m[2].replace(/\n+$/, "") };
}

function canonicalPath(rel) {
  const p = rel.replace(/\.html$/, "").replace(/\\/g, "/");
  return p === "index" ? "/" : `/${p}`;
}

function minifyCss(css) {
  return css
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/\s+/g, " ")
    .replace(/\s*([{};,])\s*/g, "$1")
    .replace(/;}/g, "}")
    .trim();
}

function jsonldFor(meta, canonical) {
  const website = {
    "@context": "https://schema.org",
    "@type": "WebSite",
    name: SITE_NAME,
    url: SITE_ORIGIN + "/",
  };
  const app = {
    "@context": "https://schema.org",
    "@type": "SoftwareApplication",
    name: SITE_NAME,
    applicationCategory: "DeveloperApplication",
    operatingSystem: "macOS, Linux",
    url: SITE_ORIGIN + "/",
    codeRepository: REPO_URL,
    license: `${REPO_BLOB}LICENSE`,
    author: { "@type": "Organization", name: "Ashlr AI" },
  };
  const isPartOf = { "@type": "WebSite", name: SITE_NAME, url: SITE_ORIGIN + "/" };
  const blocks = [];
  if (meta.schema === "home") {
    blocks.push(website, app);
  } else if (meta.schema === "article") {
    blocks.push({
      "@context": "https://schema.org",
      "@type": "TechArticle",
      headline: meta.title,
      description: meta.description,
      url: canonical,
      isPartOf,
      about: { "@type": "SoftwareApplication", name: SITE_NAME, url: SITE_ORIGIN + "/" },
    });
  } else {
    blocks.push({
      "@context": "https://schema.org",
      "@type": "WebPage",
      name: meta.title,
      description: meta.description,
      url: canonical,
      isPartOf,
    });
  }
  if (meta.crumb) {
    blocks.push({
      "@context": "https://schema.org",
      "@type": "BreadcrumbList",
      itemListElement: [
        { "@type": "ListItem", position: 1, name: SITE_NAME, item: SITE_ORIGIN + "/" },
        { "@type": "ListItem", position: 2, name: meta.crumb, item: canonical },
      ],
    });
  }
  return blocks
    .map((b) => `  <script type="application/ld+json">${JSON.stringify(b)}</script>`)
    .join("\n");
}

function lastmod(relFromSrc) {
  const abs = path.join(SRC, relFromSrc);
  const rel = path.relative(REPO_ROOT, abs);
  try {
    const out = execSync(`git -C ${JSON.stringify(REPO_ROOT)} log -1 --format=%cs -- ${JSON.stringify(rel)}`, {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    }).trim();
    if (out) return out;
  } catch {
    /* fall through */
  }
  return new Date().toISOString().slice(0, 10);
}

function* walk(dir, base = dir) {
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) yield* walk(p, base);
    else yield path.relative(base, p);
  }
}

function build() {
  const layout = fs.readFileSync(path.join(SRC, "layout.html"), "utf8");
  const css = minifyCss(fs.readFileSync(path.join(SRC, "site.css"), "utf8"));
  const out = new Map();
  const urls = [];

  for (const rel of walk(PAGES)) {
    if (!rel.endsWith(".html")) continue;
    const { meta, body } = parsePage(path.join(PAGES, rel));
    const canonical = SITE_ORIGIN + canonicalPath(rel);
    let html = layout
      .replaceAll("{{TITLE}}", esc(meta.title))
      .replaceAll("{{DESCRIPTION}}", esc(meta.description))
      .replaceAll("{{SITE_NAME}}", esc(SITE_NAME))
      .replaceAll("{{CANONICAL}}", canonical)
      .replace("{{JSONLD}}", jsonldFor(meta, canonical))
      .replace("{{CSS}}", css)
      .replace("{{CONTENT}}", body);
    // aria-current on the nav link for this top-level page
    if (meta.schema === "article" || canonicalPath(rel).startsWith("/compare")) {
      html = html.replace(`<a href="/compare/locus-vs-direnv-sops">`, `<a href="/compare/locus-vs-direnv-sops" aria-current="page">`);
    }
    out.set(rel, html);
    urls.push({ loc: canonical, lastmod: lastmod(path.join("pages", rel)) });
  }

  urls.sort((a, b) => a.loc.localeCompare(b.loc));
  const sitemap =
    `<?xml version="1.0" encoding="UTF-8"?>\n` +
    `<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n` +
    urls.map((u) => `  <url><loc>${u.loc}</loc><lastmod>${u.lastmod}</lastmod></url>\n`).join("") +
    `</urlset>\n`;
  out.set("sitemap.xml", sitemap);
  out.set("robots.txt", fs.readFileSync(path.join(SRC, "robots.txt"), "utf8").replaceAll("{{ORIGIN}}", SITE_ORIGIN));
  out.set("favicon.svg", fs.readFileSync(path.join(SRC, "favicon.svg"), "utf8"));
  return out;
}

const out = build();

if (CHECK) {
  let bad = 0;
  for (const [rel, want] of out) {
    const p = path.join(PUBLIC, rel);
    if (!fs.existsSync(p) || fs.readFileSync(p, "utf8") !== want) {
      console.error(`stale: public/${rel}`);
      bad++;
    }
  }
  for (const rel of walk(PUBLIC)) {
    if ((rel.endsWith(".html") || ["sitemap.xml", "robots.txt", "favicon.svg"].includes(rel)) && !out.has(rel)) {
      console.error(`unexpected: public/${rel}`);
      bad++;
    }
  }
  if (bad) {
    console.error(`${bad} file(s) out of sync; run: npm run build`);
    process.exit(1);
  }
  console.log(`public/ in sync (${out.size} files, origin ${SITE_ORIGIN})`);
} else {
  const stale = fs.existsSync(PUBLIC) ? [...walk(PUBLIC)] : [];
  for (const [rel, content] of out) {
    const p = path.join(PUBLIC, rel);
    fs.mkdirSync(path.dirname(p), { recursive: true });
    fs.writeFileSync(p, content);
    console.log(`wrote public/${rel}`);
  }
  for (const rel of stale) {
    if (rel.endsWith(".html") && !out.has(rel)) {
      fs.rmSync(path.join(PUBLIC, rel));
      console.log(`removed stale public/${rel}`);
    }
  }
}
