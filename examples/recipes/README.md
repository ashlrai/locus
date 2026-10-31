# Recipes: one machine, several client identities

<!-- fresh:begin pin -->
```bash
npx --yes locus-cli@0.6.0 --version
npx --yes @ashlrai/locus-mcp@0.6.0 --version
```
<!-- fresh:end -->

[run.mjs](run.mjs) runs each recipe below for real, in a disposable
`LOCUS_HOME` with fake `env:` credentials. No provider is called and your real
`~/.locus` is not touched.

```bash
cargo build --bins                                   # or install a release
node examples/recipes/run.mjs --locus "$PWD/target/debug/locus"
```

Run without `--locus` to find `locus` on `PATH`; when supplied, `--locus`
requires an absolute binary path. The runner always executes all four checks
in order: it does not provide a way to select a recipe by name. An unknown
name as the first argument (for example, `node examples/recipes/run.mjs acme`)
prints usage to stderr and exits `1` before running checks. Arguments after a
valid `--locus <absolute-path>` are currently ignored, including recipe names.
Run with `--help` or `-h` to print the invocation and exit without running
any checks.

Output from a real run (Sep 29, 2026, main and the v0.5.0 release):

```text
     child saw GH_TOKEN=fake-acme-token LOCUS_TENANT=acme-corp AWS_PROFILE=null
PASS locus run -b acme: the child gets only acme's token; ambient GH_TOKEN and AWS_PROFILE are scrubbed
PASS locus run -b personal: same command, the other identity, nothing from acme
PASS a client directory's .locus.toml refuses the wrong binding
PASS locus ci run -b acme: a short-lived CI session with the same scrubbing
COMPLETE: locus recipes (4 checks)
```

Exit `0` = all checks passed or help displayed, `1` = invalid invocation or a
check failed, `2` = binary not found (incomplete). A failed check stops the remaining checks;
the runner prints `FAIL` for a check failure or `SKIP` and `INCOMPLETE` if the
binary is missing. Only a successful run prints `COMPLETE`.

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
credential_ref = "env:ACME_GH_TOKEN"
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
#   (~/clients/acme/.locus.toml allows: acme); enter an allowed binding,
#   add `personal` to allowed_bindings in ~/clients/acme/.locus.toml, or pass --force
```

Commit `.locus.toml` so everyone working in the repo gets the same allowlist.

## 3. CI: a short-lived session per job

```yaml
- run: locus ci run -b acme -- npm test
```

`locus ci run` mints a sealed session with a TTL, applies the same scrubbing,
runs the command, and cleans up. It never writes the global `active.json` pin.
Provide the control capability and credentials as CI secrets
(`LOCUS_CONTROL_CAPABILITY`, plus values for `env:` refs). `phm:` references
currently fail closed; see [credential compatibility](../../docs/credential-compatibility.md).

## Not covered here

These recipes don't cover MCP tool scoping inside Claude Code or Cursor (see
[docs/mcp.md](../../docs/mcp.md)), approvals and dual control (see
[docs/firm-mode.md](../../docs/firm-mode.md)), or live provider calls.
