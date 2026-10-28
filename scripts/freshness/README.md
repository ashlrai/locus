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
and the newest released CHANGELOG.md heading and its `Highlights:` paragraph
before syncing. Source disagreement is an error, never silently repaired.
The changelog release heading, date and highlights are authored sources; the
summary above them is generated. Unreleased entries and historical versions
are preserved, including benchmark results and compatibility warnings.

`blocks.json` maps required files to named templates. Templates interpolate
`version`, `cli`, `mcp`, `date` and `highlights`. Add a template and register its
file, then insert `<!-- fresh:begin name -->` and `<!-- fresh:end -->` on their
own lines. Sync changes only block contents. Missing, malformed, duplicate or
unknown markers in registered files fail validation before any file is written.
Keep historical examples outside these blocks. The checker uses the same
rendering engine as sync, so there is only one freshness definition.
