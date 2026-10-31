# Conflict drivers — deterministic resolution for high-churn paths

Merge/rebase conflicts in a handful of paths are mechanical, not semantic.
This repo resolves them deterministically — no model call, no hand-merging.

| Path | Resolution |
|------|------------|
| `CHANGELOG.md` | `merge=union` in [.gitattributes](../.gitattributes) keeps both sides' hunks automatically during `git merge` / `git rebase` / `git cherry-pick`. |
| `CLAUDE.md`, `README.md` | [scripts/resolve-merge-conflicts.mjs](../scripts/resolve-merge-conflicts.mjs): take the target side's version (stage 2, "ours" during a rebase) and re-apply the incoming side's own hunks on top. Colliding hunks stay on the target's text. |
| `Cargo.lock` | Restores the target side's lockfile, then `cargo metadata` re-resolves minimally against it — never adds or upgrades deps, never touches `Cargo.toml` — and `cargo metadata --locked` verifies the result is frozen-consistent. Every `source =` must stay on crates.io. |
| `bun.lock` / `bun.lockb`, `package-lock.json` | Same shape: `bun install` + `bun install --frozen-lockfile`, or `npm install --package-lock-only` + `npm ci --dry-run`. Resolved URLs must stay on `registry.npmjs.org`. Manifests are never edited. |
| anything else | Left conflicted — fail closed; a human decides. |

## Usage

```bash
git rebase main            # CHANGELOG.md resolves itself via merge=union
# if the rebase stops on a governed path:
node scripts/resolve-merge-conflicts.mjs --continue
```

`--continue` resolves, stages, and runs `git rebase --continue` (or
`merge`/`cherry-pick --continue`), looping until the operation finishes or a
conflict it cannot resolve blocks it. `--dry-run` prints the policy each
unmerged path would get without writing. A pick that becomes fully empty is
skipped only when git itself reports the empty-pick stop.

## Caveats

- **GitHub's server-side merge ignores `.gitattributes` merge drivers** — the
  union rule only helps lane-side (local) merges and rebases. When a PR shows
  conflicts on a governed path, rebase locally and run the script.
- Lockfile conflicts on **manifest files** (`Cargo.toml`, `package.json`) are
  deliberately not covered — resolve those by hand first, then re-run.
- A resolution that fails verification (e.g. `cargo metadata --locked`
  rejects the regenerated lockfile) restores the conflict markers and exits 1
  rather than staging a bad resolution.

## Tests

```bash
node --test scripts/resolve-merge-conflicts.test.mjs
```

Each fixture is a real `git init` with synthetic content — including a rebase
of a CHANGELOG.md conflict that resolves cleanly through the union driver.
