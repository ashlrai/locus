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
- Source / Homebrew: https://github.com/ashlrai/locus
