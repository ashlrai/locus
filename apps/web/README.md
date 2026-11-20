# Locus web

Static landing page for [Locus](https://github.com/ashlrai/locus).

- **Tagline:** Wrong account, impossible.
- **First screen:** one-command install, JSON commands, MCP snippets for Claude, Cursor, and Codex, plus an illustrated pin switch.
- **Stack:** pure HTML + CSS in `public/index.html` (no build step)
- **Aesthetic:** dark monochrome terminal, sibling positioning vs [Phantom](https://phm.dev)

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

`discoverability.mjs` generates the machine-readable surface from the repo
sources of truth:

- the version: `Cargo.toml` `[workspace.package]`, which must match the newest
  dated `CHANGELOG.md` release and `apps/web/package.json`
- the published version in `scripts/freshness/published.json`, used for the
  pinned install commands, `server.json`, JSON-LD, the eyebrow label and
  `og.png`
- `README.md`: its H1, the `**Your coding agent...**` tagline, the `##`
  headings and the commands in the "Install options" fenced blocks
- every `docs/*.md` file: its H1 title and its H2-H3 headings
- `AGENTS.md`, the contact email in `SECURITY.md`, the top-level clap
  `enum Commands` in `crates/locus-cli/src/main.rs`, the MCP `control_tools()`
  names in `crates/locus-core/src/adapters/mod.rs`, and the `*.html` pages in
  `public/`

| File | Content |
|------|---------|
| `public/llms.txt` | tagline, agent start (install, smoke, MCP wiring), then a link index over README, AGENTS.md and every docs page |
| `public/llms-full.txt` | README `##` headings plus the H2-H3 heading tree of every docs/*.md |
| `llms.txt`, `llms-full.txt` (repo root) | identical copies, so an agent that clones the repo finds the same indexes |
| `public/sitemap.xml` | one `<url>` per page in `public/` under `https://locus.dev` |
| `public/robots.txt` | allow-all plus `Sitemap:` |
| `public/capabilities.json` | name/version/capabilities/install/commands/tools |
| `public/server.json` | MCP registry manifest for `io.github.ashlrai/locus-mcp` |
| `public/AGENTS.md` | repo AGENTS.md mirrored with dashes normalized |
| `public/.well-known/security.txt` | security contact, expires at release + 1 year |
| `public/og.png` | 1200x630 Open Graph card (pure-node PNG, no deps) |

`index.html` also carries `<!-- disc:KEY -->` regions: the JSON-LD
`SoftwareApplication` block and the eyebrow version label.

CI Gate runs `node apps/web/discoverability.mjs --check` and
`node --test apps/web` on every PR, so after changing any source above,
regenerate and commit the output:

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
- Keep the page intentional: monochrome, tight type, terminal snippets. Not generic AI purple.
