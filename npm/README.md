# locus-cli

<!-- fresh:begin install -->
```bash
npm install -g locus-cli@0.5.2 @ashlrai/locus-mcp@0.5.2
```
<!-- fresh:end -->

npm wrapper for the [Locus](https://github.com/ashlrai/locus) CLI — identity plane for coding agents.

**Wrong account, impossible.** Pin a binding; every command is hard-scoped to that tenant until you re-pin.

## Install

```bash
npm install -g locus-cli
# or
npx locus-cli --help
```

On first run the wrapper:

1. Downloads the matching GitHub release binary into `~/.locus/bin`, or
2. Falls back to `cargo install --git https://github.com/ashlrai/locus locus-cli --locked`. On platforms
   without a prebuilt release binary (today: Linux arm64 and Windows) it says so
   and goes straight to this step, which can take several minutes.

Requires Node ≥ 16. Cargo fallback needs [Rust](https://rustup.rs).
The fallback builds in a temporary private install root and installs the result
in `~/.locus/bin`; it does not replace binaries in Cargo's install directory.
It compares the built binary's actual version with the cache before replacement.

The wrapper refuses to overwrite a newer `~/.locus/bin/locus` with its older
release, including during npm postinstall. It also preserves an existing binary
whose version cannot be determined. The refusal reports both versions when known.
To intentionally replace that binary, run the wrapper with a leading `--force`,
for example `locus --force --version`, or run `node install.js --force` from the
installed `locus-cli` package directory. The wrapper consumes this leading flag;
flags after a command, such as `locus leave --force`, belong to the native CLI
and never authorize a downgrade. npm's own `--force` is not an override.

## Quick start

```bash
locus quickstart                 # sample bindings, pin, whoami, doctor
eval "$(locus hook bash)"        # or zsh; exports the control capability
locus whoami
locus exec -- env | grep LOCUS_
```

Step by step instead of `quickstart`: `locus init --with-samples`, then
`eval "$(locus hook bash)"`, then `locus pin personal`. Without the hook line,
`pin` fails with `LOCUS_CONTROL_CAPABILITY is required`.

## Related

- MCP server: [`@ashlrai/locus-mcp`](https://www.npmjs.com/package/@ashlrai/locus-mcp)
- Pairs with Phantom Secrets — the secret plane to Locus's identity plane: https://github.com/ashlrai/locus#how-locus-plugs-into-phantom
- Source / Homebrew: https://github.com/ashlrai/locus
