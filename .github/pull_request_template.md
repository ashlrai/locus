## What this changes

<!-- One or two sentences. Link the issue: "Closes #123". Keep PRs focused: one adapter, one CLI surface, or one isolation fix. -->

## Isolation property

<!-- Which isolation property changes, or "none". Link the relevant DESIGN.md section for security-sensitive code. -->

## How I tested it

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace`

## Checklist

- [ ] Fails closed: invalid seals, scope mismatches, and unknown providers deny instead of allowing
- [ ] No raw tokens in bindings (CredentialRefs only), and nothing new printed to `locus-mcp` stdout
- [ ] Docs updated if a CLI flag, adapter, or MCP tool changed

<!-- Security vulnerabilities: do not open a PR or issue. See SECURITY.md. -->
