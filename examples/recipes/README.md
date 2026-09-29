# Recipes: one machine, several client identities

[run.mjs](run.mjs) runs each recipe below for real, in a disposable
`LOCUS_HOME` with fake `env:` credentials. No provider is called and your real
`~/.locus` is not touched.

```bash
cargo build --bins                                   # or install a release
node examples/recipes/run.mjs --locus "$PWD/target/debug/locus"
```

Output from a real run (Sep 29, 2026, main and the v0.5.0 release):

```text
     child saw GH_TOKEN=fake-acme-token LOCUS_TENANT=acme-corp AWS_PROFILE=null
PASS locus run -b acme: the child gets only acme's token; ambient GH_TOKEN and AWS_PROFILE are scrubbed
PASS locus run -b personal: same command, the other identity, nothing from acme
PASS a client directory's .locus.toml refuses the wrong binding
PASS locus ci run -b acme: a short-lived CI session with the same scrubbing
COMPLETE: locus recipes (4 checks)
```

Exit `0` = all passed, `1` = a check failed, `2` = binary not found (incomplete).

## 1. Run a command as exactly one client

Two bindings, each with its own GitHub credential reference:

```toml
# ~/.locus/bindings/acme.toml
[binding]
id = "bnd_acme"
alias = "acme"
tenant = "acme-corp"

[[binding.providers]]
provider = "github"
account = "acme-corp"
credential_ref = "phm:GH_TOKEN_ACME"      # recipe uses env:ACME_GH_TOKEN
scope = { orgs = ["acme-corp"], repos = ["acme-corp/*"] }
```

```bash
locus run -b acme -- gh repo list          # GH_TOKEN = acme's, LOCUS_TENANT=acme-corp
locus run -b personal -- gh repo list      # the other identity; nothing from acme
```

The shell's own `GH_TOKEN` (the one your last `gh auth login` left behind) and
`AWS_PROFILE` are scrubbed from the child. It gets only the binding's resolved
credential and `LOCUS_*` metadata. `locus run` is one-shot and leaves the
global pin alone. Use `locus pin` / `locus enter` for a session.

## 2. Make a client directory refuse the wrong identity

```bash
cd ~/clients/acme
locus workspace --default acme --allow acme --require-pin   # writes .locus.toml
locus run -b personal -- git push
# error: create run session for `personal`: binding `personal` is not allowed in this workspace
```

Commit `.locus.toml` so everyone working in the repo gets the same allowlist.

## 3. CI: a short-lived session per job

```yaml
- run: locus ci run -b acme -- npm test
```

`locus ci run` mints a sealed session with a TTL, applies the same scrubbing,
runs the command, and cleans up. It never writes the global `active.json` pin.
Provide the control capability and credentials as CI secrets
(`LOCUS_CONTROL_CAPABILITY`, plus `env:` refs or Phantom).

## Not covered here

These recipes don't cover MCP tool scoping inside Claude Code or Cursor (see
[docs/mcp.md](../../docs/mcp.md)), approvals and dual control (see
[docs/firm-mode.md](../../docs/firm-mode.md)), or live provider calls.
