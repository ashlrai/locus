# DeepWiki auto-docs

[DeepWiki](https://deepwiki.com) generates a browsable, AI-written wiki for
this repository. The file [`.devin/wiki.json`](../.devin/wiki.json) at the
repository root steers that generation: with a `pages` list present, DeepWiki
skips its default cluster-based planning and generates **exactly the pages we
list** — nothing more, so the list is the whole wiki, not an overlay.

## Contract

Both top-level fields are required; `repo_notes` may be an empty array,
`pages` must list at least one page.

| Field | Shape | Notes |
| ----- | ----- | ----- |
| `repo_notes[]` | `{ "content": string, "author"?: string }` | Priorities and context for the whole repo. Max 10,000 characters per note. |
| `pages[]` | `{ "title": string, "purpose": string, "parent"?: string\|null, "page_notes"?: note[] }` | One entry per generated page. `title` must be unique and non-empty; `parent` is the title of another page for hierarchy. |

Limits: **30 pages**, **100 notes total** (`repo_notes` plus every
`page_notes`), 10,000 characters per note.

## How this repo uses it

- One page per plane, crate, or operational area (`Architecture`, `locus-core`,
  `Policy and Approvals`, …), with `parent` links grouping the detail pages
  under them.
- `repo_notes` carries the rules generated docs must not get wrong: identity
  is resolved at the gate, agents cannot pin, CredentialRefs only — never raw
  tokens.
- Purposes name the files that back each page (`docs/mcp.md`,
  `crates/locus-core/src/isolation.rs`), so the wiki cites real sources
  instead of restating them.

## Maintenance

The config only takes effect when DeepWiki (re)generates the wiki, so a stale
file fails silently — hence the guard:

```bash
node --test scripts/devin-wiki.test.mjs
```

It enforces the contract above plus hierarchy sanity (every `parent` resolves,
no cycles) and that every repository path named in the file still exists. It
runs with the other [docs checks](../CONTRIBUTING.md) (Node 18+, no
dependencies) and is listed there.

When adding a crate, a top-level surface, or a major doc, add or adjust the
page in the same change. Keep titles unique and treat the 30-page ceiling as
a prompt to merge pages, not to drop content.
