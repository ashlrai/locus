# Locus web

Static landing page for [Locus](https://github.com/ashlrai/locus).

- **Tagline:** AI-native identity plane · Wrong account, impossible.
- **v0.2 surfaces:** local dashboard, forensics, HTTP MCP, goal status
- **Stack:** pure HTML + CSS in `public/index.html` (no build step)
- **Aesthetic:** dark monochrome terminal — sibling positioning vs [Phantom](https://phm.dev)

## Local

```bash
cd apps/web
npm start
# → http://localhost:3000
```

Or without npm:

```bash
npx serve public -l 3000
# or
python3 -m http.server 3000 --directory public
```

## Deploy

The site is the contents of `public/`. Set the publish directory to `public` (or `apps/web/public` from the monorepo root).

### Vercel

```bash
# from apps/web
npx vercel --prod
```

Or in the Vercel dashboard / `vercel.json`:

| Setting | Value |
|---------|--------|
| Root Directory | `apps/web` |
| Build Command | *(none)* |
| Output Directory | `public` |
| Framework | Other |

Example `vercel.json` (optional, place in `apps/web/`):

```json
{
  "version": 2,
  "public": true,
  "cleanUrls": true,
  "trailingSlash": false
}
```

Vercel static: set **Output Directory** to `public` with an empty build command.

### Cloudflare Pages

| Setting | Value |
|---------|--------|
| Build command | *(leave empty)* or `echo static` |
| Build output directory | `public` |
| Root directory | `apps/web` (if connecting the monorepo) |

Direct upload:

```bash
npx wrangler pages deploy public --project-name=locus
```

### GitHub Pages

Publish `apps/web/public` via Actions or a `gh-pages` branch that contains only those files. Enable Pages → Deploy from branch / folder.

### Netlify

| Setting | Value |
|---------|--------|
| Base directory | `apps/web` |
| Publish directory | `public` |
| Build command | *(empty)* |

## Agent discoverability

`discoverability.mjs` generates the machine-readable surface under `public/`
from the repo sources of truth (Cargo.toml version, CHANGELOG release date,
README install blocks, docs/*.md headings, the clap `enum Commands` and the
MCP `control_tools()` list):

| File | Content |
|------|---------|
| `public/llms.txt` | H1 + tagline + link index over README and every docs page |
| `public/llms-full.txt` | full heading tree of README and docs/*.md |
| `public/sitemap.xml` | one `<url>` per page in `public/` under `https://locus.dev` |
| `public/robots.txt` | allow-all plus `Sitemap:` |
| `public/capabilities.json` | name/version/capabilities/install/commands/tools |
| `public/server.json` | MCP registry manifest for `io.github.ashlrai/locus-mcp` |
| `public/AGENTS.md` | repo AGENTS.md mirrored with dashes normalized |
| `public/.well-known/security.txt` | security contact, expires at release + 1 year |
| `public/og.png` | 1200x630 Open Graph card (pure-node PNG, no deps) |

`index.html` also carries `<!-- disc:KEY -->` regions: the JSON-LD
`SoftwareApplication` block and the eyebrow version label.

```bash
cd apps/web
npm run generate:agents   # regenerate everything
npm run check:agents      # exit 1 when any generated file is stale
npm test                  # node --test (discoverability assertions)
```

The canonical URL is `https://locus.dev/`; the sitemap, robots, OpenGraph and
JSON-LD all point there. The site itself stays a no-build static deploy.

## Notes

- No secrets, no analytics, no framework lock-in.
- Links point at `https://github.com/ashlrai/locus` and `https://phm.dev`.
- Keep the page intentional: monochrome, tight type, terminal snippets — not generic “AI purple.”
