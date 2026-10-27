# Locus web

Static site for [Locus](https://github.com/ashlrai/locus).

- **Tagline:** AI-native identity plane · Wrong account, impossible.
- **v0.2 surfaces:** local dashboard, forensics, HTTP MCP, goal status
- **Stack:** HTML in `src/pages/` + shared `src/layout.html` and `src/site.css`, assembled by `scripts/build.mjs` into `public/` (committed; no framework)
- **Pages:** `/` plus `/use-cases/*` intent pages and `/compare/locus-vs-direnv-sops`; every factual claim block carries a `data-claim` source link into the repo's docs or code

## Local

```bash
cd apps/web
npm run build   # regenerate public/ from src/
npm test        # acceptance: claim sources, canonicals, sitemap, links
npm start       # serve public/ on http://localhost:3000
```

`npm start` uses `serve`, which resolves extensionless URLs like `/use-cases/mcp-tool-scoping`.

## Site origin

`site.config.mjs` holds the public origin used for canonicals, `sitemap.xml`, `robots.txt`, and OG tags. Default: `https://locus.ashlr.ai`. Override for staging:

```bash
LOCUS_SITE_ORIGIN=https://preview.example.com npm run build
```

## Sitemap

`npm run build` writes `public/sitemap.xml`: one `<url>` per page with an absolute `<loc>` equal to the page canonical and `<lastmod>` taken from the file's last git commit (build date for uncommitted sources). `npm test` fails if `public/` drifts from `src/`.

## Deploy

The site is the contents of `public/`. `vercel.json` ships `cleanUrls: true` and `trailingSlash: false`; keep whatever host you use consistent with extensionless canonicals.

### Vercel

| Setting | Value |
|---------|--------|
| Root Directory | `apps/web` |
| Build Command | `npm run build` (or none; `public/` is committed) |
| Output Directory | `public` |
| Framework | Other |

### Cloudflare Pages

| Setting | Value |
|---------|--------|
| Build command | `npm run build` or empty |
| Build output directory | `public` |
| Root directory | `apps/web` (if connecting the monorepo) |

### GitHub Pages / Netlify

Publish `apps/web/public`. GitHub Pages resolves `/x` to `x.html`; on Netlify rely on its default pretty-URL behavior.

## Notes

- No secrets, no analytics, no framework lock-in.
- Links point at `https://github.com/ashlrai/locus` and `https://phm.dev`.
- Keep the site monochrome and tight: terminal snippets, measured claims, sources on every `data-claim` block.
