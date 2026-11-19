## What this changes

<!-- One or two sentences. Link the issue: "Closes #123". Keep PRs focused: one adapter, one CLI surface, or one isolation fix. -->

## Isolation property

<!-- Which isolation property changes, or "none". Link the relevant DESIGN.md section for security-sensitive code. -->

## How I tested it

- [ ] `git fetch origin && bash scripts/fast-check.sh` passes (fmt, plus clippy `-D warnings` and tests for the crates this PR touches)
- [ ] Docs changes: the docs checks in CONTRIBUTING.md ("Test") pass, and `node apps/web/discoverability.mjs --check` passes (run it without `--check` to regenerate)
- [ ] Any local smoke used a throwaway `LOCUS_HOME`, never a real `~/.locus`

## Checklist

- [ ] Fails closed: invalid seals, scope mismatches, and unknown providers deny instead of allowing
- [ ] No raw tokens in bindings (CredentialRefs only), and nothing new printed to `locus-mcp` stdout
- [ ] Docs/README updated if a CLI flag, adapter, MCP tool, or other behavior changed

## Notes for reviewers

<!-- Anything risky, breaking, or worth a closer look -->

<!-- Security vulnerabilities: do not open a PR or issue. See SECURITY.md. -->
