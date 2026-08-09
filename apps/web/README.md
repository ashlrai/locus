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

## Notes

- No secrets, no analytics, no framework lock-in.
- Links point at `https://github.com/ashlrai/locus` and `https://phm.dev`.
- Keep the page intentional: monochrome, tight type, terminal snippets — not generic “AI purple.”
