# Release freshness

This Cargo repository has no root package.json. With Node 18 or newer, run:

```sh
node scripts/freshness/sync.mjs    # fresh:sync
node scripts/freshness/check.mjs   # fresh:check (read only, exit 1 on drift)
node --test scripts/freshness/freshness.test.mjs
```

Both commands work from any directory. No dependencies, credentials or network
access are required. CI can call check directly; this change adds no workflows.

Update `[workspace.package].version` in Cargo.toml, both npm wrapper versions,
and the newest source CHANGELOG.md heading and its `Highlights:` paragraph
before syncing. Source disagreement is an error, never silently repaired.
The changelog release heading, date and highlights are authored sources; the
summary above them is generated. Unreleased entries and historical versions
are preserved, including benchmark results and compatibility warnings.

`blocks.json` maps required files to named templates. Templates interpolate
`version` (source), `publishedVersion`, `cli`, `mcp`, `date` and `highlights`. Add a template and register its
file, then insert `<!-- fresh:begin name -->` and `<!-- fresh:end -->` on their
own lines. Sync changes only block contents. Missing, malformed, duplicate or
unknown markers in registered files fail validation before any file is written.
Keep historical examples outside these blocks. The checker uses the same
rendering engine as sync, so there is only one freshness definition.

Package READMEs use the version-neutral `package-install` template because
they are embedded in immutable npm tarballs. Public documentation can use
the published-version `install` template without baking an old pin into a
future package.

## Source candidate versus published release

`scripts/freshness/published.json` pins the last verified published version.
Public install commands, badges and latest-release summaries use that version;
its date and highlights come from its existing dated changelog section. Cargo,
package manifests and the newest changelog section must still agree on the
source candidate version. A source bump alone never changes the published pin.

After the release owner has verified the tagged GitHub assets and **both** npm
packages, update `published.json` to that version, run `sync.mjs`, and review and
run `node apps/web/discoverability.mjs`,
and publish the resulting documentation update. The generator does not query a
registry, create tags or prove publication; this is an explicit release-owner
step, not hidden automation. Keep the old pin if any publication stage is pending.

The root `server.json` is the source/release descriptor and continues to match
Cargo and npm source versions. It is not the public website install descriptor:
`apps/web/public/server.json` uses the verified published version instead.
Do not rewrite the root descriptor when advancing only the published pointer.
