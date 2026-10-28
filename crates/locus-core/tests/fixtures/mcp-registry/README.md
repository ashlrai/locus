# MCP Registry schema fixture

`server.schema.json` is an unmodified copy of the published MCP Registry schema:

- Source: https://static.modelcontextprotocol.io/schemas/2025-12-11/server.schema.json
- Retrieved: 2026-10-26
- SHA-256: `3fba09590c99f61735d234822279f4223fab9e300c0a81e81c91ab62a4114de0`

`LICENSE` preserves the upstream licensing notice retrieved from
https://github.com/modelcontextprotocol/registry/blob/main/LICENSE on the same
date, including its MIT to Apache-2.0 transition terms.

The root `server.json` declares this dated schema. The `registry_manifest`
integration tests validate it offline, check versions against
`Cargo.toml`'s `workspace.package.version` and `npm-mcp/package.json`, and
check the existing npm stdio entrypoint. Core already has the schema and
TOML dependencies, so no extra test tools or publication changes are needed.

Run from the repository root with an isolated HOME:

```bash
RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}" \
CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}" \
HOME="$(mktemp -d)" cargo test -p locus-core --test registry_manifest --locked
```

On schema upgrades, download the new dated URL without modifying its contents,
update the root manifest's `$schema`, and record the new digest here. A schema
or test update does not submit the server to a registry. Public submission
requires Mason's approval.
