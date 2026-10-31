<div align="center">

<img src="https://raw.githubusercontent.com/ashlrai/locus/main/apps/web/public/og.png" alt="Locus — wrong account, impossible. Identity plane for coding agents." width="720" />

<h1>Locus</h1>

**Your coding agent can't act as the wrong account.**

**The identity plane for coding agents · by [Ashlr](https://ashlr.ai).**

Pin a client, and every CLI command, MCP tool and agent session is sealed to that identity until you re-pin. No more ambient `gh auth`, a shared Supabase token or "whichever Vercel team was last".

[**30-second start**](#30-second-start) ·
[**Install**](#install-options) ·
[**Plugs into Phantom**](#how-locus-plugs-into-phantom) ·
[**Security model**](#security-model) ·
[**Docs**](#docs)

<!-- fresh:begin badge -->
[![Release 0.5.2](https://img.shields.io/badge/release-v0.5.2-blue?style=for-the-badge&labelColor=0b0b14)](https://github.com/ashlrai/locus/releases/tag/v0.5.2)
<!-- fresh:end -->
[![CI](https://img.shields.io/github/actions/workflow/status/ashlrai/locus/ci.yml?style=for-the-badge&label=CI&logo=github&labelColor=0b0b14)](https://github.com/ashlrai/locus/actions/workflows/ci.yml)
[![npm: locus-cli](https://img.shields.io/npm/v/locus-cli?style=for-the-badge&logo=npm&label=locus-cli&labelColor=0b0b14)](https://www.npmjs.com/package/locus-cli)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg?style=for-the-badge&labelColor=0b0b14)](./LICENSE)
[![Rust ≥1.89](https://img.shields.io/badge/rust-%E2%89%A51.89-CE412B?style=for-the-badge&logo=rust&labelColor=0b0b14)](./rust-toolchain.toml)

</div>

<!-- fresh:begin release -->
Latest release: [v0.5.2](https://github.com/ashlrai/locus/releases/tag/v0.5.2) (2026-10-26).

`locus renew` and expired-pin recovery (#193), the global `--dry-run` preview, exact-loopback HTTP (#187) and index URL redaction (#185).
<!-- fresh:end -->

![locus quickstart pins a sample binding; locus whoami shows the sealed session and its scoped providers](docs/assets/locus-demo.gif)

## 30-second start

```bash
npm install -g locus-cli @ashlrai/locus-mcp   # or brew or cargo; see Install options
locus quickstart                        # sample bindings, pin, whoami, doctor
eval "$(locus hook zsh)"                # or bash; exports the control capability
locus enter personal                    # pin from this shell (it supervises the pin)
locus whoami                            # who your agents are acting as, right now
locus exec --expect personal --no-resolve -- echo ok # runs: personal is pinned
locus exec --expect acme -- echo ok     # refused, exit 1, child never starts (pin_mismatch)
```

The `echo` smoke test uses `--no-resolve` because it needs no credentials.
Existing bindings are not replaced by quickstart; `--no-resolve` refuses bindings
with credential-resolving upstreams. For real provider work, supply credentials
outside agent context and replace the sample scopes. Missing credentials still
produce warnings when resolution is enabled. See the [sample exec
walkthrough](examples/sample-exec-q8.md).

What the two extra steps are for:

- **Control capability.** Control commands (`enter`, `pin`, `leave`, `exec`) need `LOCUS_CONTROL_CAPABILITY` in the shell. `locus quickstart` mints it and saves it at `~/.locus/control_capability` (0600, respects `LOCUS_HOME`); `eval "$(locus hook zsh)"` exports it without printing it. Without it, `locus exec` stops with `LOCUS_CONTROL_CAPABILITY is required for control-plane authority`. `locus capability status` shows the posture without the value.
- **Broker pin.** A pin is backed by an authority broker that the pinning shell supervises. `locus enter personal` works straight after `quickstart`, including when quickstart ran in a shell that has already exited (fixed in #156, regression test `crates/locus-cli/tests/quickstart_then_enter.rs`).
- **Recovery.** If the pinning shell exits, follow the [dead-broker recovery steps](#recovering-after-the-broker-exits) in a new operator shell.

### Recovering after the broker exits

If a command reports `human re-pin required` or `no current broker endpoint`,
open a new operator shell, change to the intended workspace, and use the same
Locus home as the affected session. Load the hook and try entering the intended
binding first; an intact local-control active pin whose broker died can be
replaced directly when no session ID is explicitly selected:

```bash
cd /path/to/workspace
eval "$(locus hook bash)"             # use zsh for a zsh shell
locus enter personal                  # replace personal with your binding alias
locus whoami
```

`locus enter --force` is a different option: it allows bindings outside the
workspace allowlist and is not needed to replace an intact dead-broker pin.

If `enter` is refused, read the error before continuing. For a wedged session,
such as an expired pin, invalid or legacy seal, or unreadable session, a human
can clear it and re-enter in the same hooked shell:

```bash
locus leave --force
locus enter personal
locus whoami
```

Forced leave requires the control capability, verified against the live broker
or persisted operator capability; it deletes session state only and never
mints a pin. Do not use it to bypass a capability or workspace refusal. The
exceptional `locus leave --force --no-verifier` acknowledges teardown without
capability verification only when neither a live broker nor a persisted
operator capability is available. It cannot override a live broker's refusal
or a mismatching persisted capability. A subsequent `enter` still requires
normal authorization. See `locus leave --help` and the
[broker recovery guide](docs/broker-recovery-q8.md) for details. Keep the shell
that enters the pin alive while using its broker.

### Fresh quickstart doctor warning

[Issue #172](https://github.com/ashlrai/locus/issues/172) recorded a fresh
quickstart on npm 0.5.1: `quickstart` succeeded, but a subsequent `doctor`
returned exit 1 with `WARN` when Phantom was absent. To reproduce the isolated
setup with an installed `locus` binary, run the following in Bash. It uses a
temporary home and an empty project, hides optional executables from `PATH`,
and starts each Locus command with a clean environment:

```bash
cli_bin="$(command -v locus)"
work="$(mktemp -d)"
mkdir -p "$work/home" "$work/project"
cd "$work/project"
env -i HOME="$work/home" PATH=/nonexistent SHELL=/bin/bash NO_COLOR=1 "$cli_bin" quickstart > /dev/null
printf 'quickstart exit: %s\n' "$?"
env -i HOME="$work/home" PATH=/nonexistent SHELL=/bin/bash NO_COLOR=1 "$cli_bin" doctor > "$work/doctor.out"
doctor_status=$?
grep -E 'verdict   WARN|\[phantom_missing\]' "$work/doctor.out"
printf 'doctor exit: %s\n' "$doctor_status"
```

On the pre-fix 0.5.1 source, quickstart exited 0 and the selected doctor output
was (other findings omitted):

<!-- fresh-doctor-output:begin -->
```text
  verdict   WARN
  ! [phantom_missing] phantom not on PATH (install Phantom Secrets for Phantom credential references)
```
<!-- fresh-doctor-output:end -->

Doctor exited 1. Since the fix included in 0.5.2, the same setup still reports
the warning but exits 0 when findings are only setup advisories. The JSON
report still has `verdict: "WARN"` and `ok: false`; exit 0 does not mean all
setup is complete. Inspect the full `locus doctor --json` report for other
findings: actionable warnings can still exit 1, and unsafe conditions exit 2.
Phantom is optional unless using its credential references; its scoped bridge
is not available in this checkout. To export the persisted control capability
in your normal shell, use the hook described above. See the [first-run
record](docs/FIRST-RUN.md) for the original observation.

## Connect your agent (MCP)

`locus agent setup --apply` writes the configs for every client it finds. By hand:

```bash
claude mcp add locus -- locus mcp --supervised    # Claude Code
codex mcp add locus -- locus mcp --supervised     # Codex
```

Cursor (`~/.cursor/mcp.json`):

```json
{ "mcpServers": { "locus": { "command": "locus", "args": ["mcp", "--supervised"] } } }
```

`locus mcp --supervised` serves the pinned binding's provider tools; launch the client from the shell that ran `locus hook` and `locus pin`, and restart it after a re-pin. Registered as plain `locus-mcp` (what `locus setup` writes), the server serves control tools only (pin status, verification, `locus_request_pin`). Neither returns credential values. See [docs/mcp.md](./docs/mcp.md#supervised-launch-provider-tools).

### How it compares

| Approach | Where the identity lives | Reach of a switch |
|---|---|---|
| `gh auth switch` | gh's shared config (`hosts.yml`) | Every process that reads that config, until the next switch |
| direnv `.envrc` | Variables exported into your shell on `cd` | Only the variables in `.envrc`; the rest of the shell's environment stays |
| 1Password `op run` | Secret references resolved for one command | That command |
| **Locus** `run` / `exec` / `pin` | A sealed session bound to one binding | That command or session. Ambient `GH_TOKEN`, `AWS_*`, `SUPABASE_*`… are scrubbed, a directory's `.locus.toml` can refuse other bindings, and MCP provider tools (`locus mcp --supervised`) are only the pinned binding's |

Use explicit environment (`env:`) references for supported credential resolution. Phantom (`phm:`) references remain readable for metadata and migration, but require a scoped bridge that is not available in this source checkout. Try it in a minute with the executable [recipes](./examples/recipes/) (two clients, a directory allowlist, a CI session).

---

## Install options

Homebrew, npm, and Cargo are the current installation options:

```bash
# Homebrew (tap live) — installs locus + locus-mcp
brew install ashlrai/tap/locus

# npm (release binary, or cargo if this platform has no asset)
npm install -g locus-cli @ashlrai/locus-mcp
npx locus-cli --help
npx @ashlrai/locus-mcp   # MCP server for Claude Code / Cursor

```

### Proposed shell installer

[Issue #169](https://github.com/ashlrai/locus/issues/169) proposes a
one-command installation path for machines without npm or Rust. The repository
contains an [installer implementation](./scripts/install.sh), but this proposal
does not establish that a published one-command installation path or matching
assets are available for every release. Use the current options above or the
source install below in the meantime.

The intended installer selects a platform-specific `locus-<target>.tar.gz`
release archive and its required `.sha256` sidecar, verifies the archive, and
installs both `locus` and `locus-mcp`. Missing or mismatched checksums must stop
installation before binaries are replaced; checksums detect corruption but are
not publisher signatures. The implementation targets macOS Apple Silicon and
Intel, and Linux x86_64 with glibc. On unsupported platforms, including Linux
ARM, try a source install. It requires Bash, curl, tar, and `sha256sum` or `shasum`;
its default destination is `~/.local/bin`, without sudo or shell profile edits.
Users must add the destination to their `PATH` themselves.

### Install from source

The minimum supported Rust version (MSRV) for source installs, including npm's
Cargo fallback, is **Rust 1.89 or newer**, with a matching Cargo toolchain. The
[workspace manifest](./Cargo.toml) declares this as `rust-version` for all crates.
With Rust 1.83, installation fails before compiling Locus because the locked
`idna_adapter` dependency uses the 2024 edition, which that Cargo cannot parse
([issue #167](https://github.com/ashlrai/locus/issues/167)). Cargo 1.85 through
1.88 can parse it but reject the workspace's minimum Rust version.

Check the active toolchain before installing:

```bash
rustc --version
cargo --version
```

If necessary, update and select a supported toolchain with `rustup update stable`
and `rustup default stable`. A checkout's `rust-toolchain.toml` selects stable
inside the checkout, but it does not select the toolchain for `cargo install
--git` run elsewhere. Check the versions in the directory where you install.

```bash
# Install both binaries from Git
export PATH="$HOME/.cargo/bin:$PATH"
cargo install --git https://github.com/ashlrai/locus locus-cli --locked
```

Or install from a local checkout:

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo install --locked --path crates/locus-cli
```

Homebrew formula (for taps): [integrations/homebrew](./integrations/homebrew).

---

## More of the CLI

```bash
# First 60 seconds
locus quickstart                 # samples · enter · whoami · doctor
# quickstart also mints the operator control capability if missing, persists it
# 0600 at ~/.locus/control_capability (respects LOCUS_HOME), and adopts it for
# the run. Export it in new shells (value never echoed):
eval "$(locus hook zsh)"
# Manual alternative: export LOCUS_CONTROL_CAPABILITY="$(openssl rand -hex 32)"

# Or explicit pin
locus init --with-samples
locus enter personal && locus whoami

# Switch client — previous identity is gone from the process env
locus switch acme && locus whoami

# Local identity dashboard (loopback UI + API)
locus dashboard                  # http://127.0.0.1:8750

# Check the pinned binding without resolving credentials
locus exec --no-resolve -- true

# Wire AI clients + hub readiness
locus agent setup --apply
locus agent report --json        # ready | protected | unsafe

# Directory-local default
cd ~/clients/acme
locus workspace --default acme --allow acme,acme-ro --require-pin
locus pin          # uses .locus.toml
```

### Shell prompt

```bash
eval "$(locus hook zsh)"
# shows [locus:acme:acme-corp] or [locus:unpinned]
# also exports the persisted LOCUS_CONTROL_CAPABILITY when the shell lacks one
```

### Previewing mutations with `--dry-run`

[Issue #171](https://github.com/ashlrai/locus/issues/171) covers previews for
mutating CLI commands. The following command forms support a plan in this
checkout; supply their normal required arguments:

| Area | Command forms |
| --- | --- |
| Setup and sessions | `init`, `quickstart`, `onboard`, `setup`, `workspace`, `enter`, `switch`, `pin`, `leave`, `renew` |
| Bindings and workspaces | `engagement init`, `engagement close`, `graph export`, `graph import`, `binding add`, `binding rm`, `binding migrate-credential-refs`, `client add`, `offboard`, `autopin suggest --apply` |
| Capabilities and grants | `capability persist`, `capability unpersist`, `ci mint`, `ci env`, `mcp mint`, `mcp revoke` |
| Setup and adapters | `agent setup`, `doctor --fix`, `adapter install`, `adapter uninstall`, `adapter trust add`, `adapter registry export`, `adapter registry index add`, `adapter registry index remove` |
| Approvals and audit | `approve grant`, `approve deny`, `notify on`, `notify off`, `events export`, `forensics export` |

For example, `locus init --dry-run --json` previews initialization. A plan should:

- Perform read-only preflight checks and describe the proposed changes; JSON
  plans use the `locus.plan.v1` schema.
- Leave persistent state unchanged, including the Locus home, session and
  binding data, workspace and client configuration, and export files. Do not
  start a child or server, resolve credentials, or perform provider mutations.
- Report failed preconditions without applying changes. A successful preview
  does not authenticate the control capability or guarantee that a later real
  command will be authorized or succeed.

`doctor --fix` previews repairs by default; `--apply` writes them. Likewise,
`binding migrate-credential-refs` previews by default and `--write` persists
changes. `setup --print` and `setup --client generic` remain print-only, even
with the flag. Adapter install and registry index add previews do not fetch or
verify remote content.

`doctor --fix --apply --dry-run`, `agent setup --apply --dry-run`, and
`pin --rewrite-configs --dry-run` are rejected. `exec`, `run`, `ci run`, the
`mcp` server (without a subcommand), `serve`, and `dashboard` cannot produce
plans. `locus exec --dry-run -- <command>` exits 1; other placements and the
other listed commands reject the flag with exit 2. Arguments after `--` belong
to the child. Child commands, provider operations, and MCP tool calls are
outside this preview's scope.

Read-only commands accept and ignore the flag, so their normal behavior can
still create a Locus home or freeze a drifted session. See the [dry-run command
matrix](docs/dry-run-matrix-q8.md) for all forms and limitations.

---

## Measured: wrong-client credential reachability

Scenario: your shell still holds another client's ambient credentials (the `gh auth switch` or direnv leftover) and you launch a child process. Which of those can it still reach? Reproduce with `python3 eval/env-reachability/redteam.py` (needs `locus` 0.5.0 on PATH; fake canary credentials, throwaway `HOME` and `LOCUS_HOME`).

| launch | env credentials reachable (16 variables x 3 trials) |
| --- | --- |
| plain launch | 48 of 48 |
| `locus run -b acme -- <cmd>` | **0 of 48** |

Also measured: with locus, credential files read through `$HOME` (`~/.config/gh/hosts.yml`, `~/.aws/credentials`, `~/.netrc`, `~/.git-credentials`, `~/.npmrc`) were not reachable (0 of 15), and an agent attempt to pin a binding was refused (3 of 3).

**This is not a sandbox.** The same files read by their absolute real path stayed reachable under locus (15 of 15, same as a plain launch). Locus scrubs the environment and redirects `HOME`; it does not stop a process from opening files it already has OS permission to open. Scripted adversary, one machine, locus 0.5.0, 3 trials, no external audit.

## How isolation works

```
locus pin acme
        │
        ▼
  Session sealed (HMAC) to binding "acme"
        │
        ├─► locus-mcp   → tools = only this binding (+ locus_whoami)
        │                 agents cannot pin (locus_request_pin only)
        │                 scope freeze: wrong project_ref → deny
        │
        └─► locus exec -- <cmd>
              ├─ scrubs ambient AWS_PROFILE / GH_TOKEN / SUPABASE_* / …
              ├─ resolves explicit env: credential_refs into provider env vars
              ├─ private GH_CONFIG_DIR + AWS_* under ~/.locus/workers/<session>/
              └─ never injects other bindings' providers
```

**Credential refs**

| Ref | Resolution |
|-----|------------|
| `phm:NAME` | Reserved pointer; resolution fails before any Phantom invocation until a supported scoped bridge exists |
| `env:VAR` | Explicitly supplied parent process variable, injected only into the pinned provider’s child environment |

See [credential compatibility and onboarding](./docs/credential-compatibility.md) for the supported setup and release distinction.

`test:` credentials are compiled-test-only and are always rejected by production binaries, regardless of environment.

Schemes are mandatory: bare names, raw tokens, empty refs, and unsupported schemes are rejected when bindings are saved or loaded. Never put raw secrets in binding files.

Legacy bare Phantom names are never silently skipped. Inspect the safe dry run with `locus binding migrate-credential-refs <alias>`, then persist conservative conversions with `--write`. Unsafe values require manual editing and are never printed.

## MCP (Claude Code / Cursor)

```bash
# after Install (cargo / brew / npm)
locus enter acme
locus agent setup --apply      # MCP configs + AGENT.md (or: locus setup --client claude)
# restart Claude Code — tools: locus_whoami, supabase.scope, … + resources/prompts

# Optional: HTTP MCP for CI agents (loopback + token)
LOCUS_MCP_HTTP_TOKEN=secret locus-mcp --http 127.0.0.1:8742
# POST /mcp  ·  GET /health
```

MCP identity and provider-scope responses report only credential presence and source (`phantom` or `environment`); they never return `credential_ref` values or resolved secrets.

---

## How Locus plugs into Phantom

Locus and [Phantom Secrets](https://phm.dev/secrets) are the two halves of the Ashlr agent-safety pair — different questions, different planes:

| Plane | Tool | Question it answers |
|-------|------|---------------------|
| Secret plane | `phantom` ([phantom-secrets](https://github.com/ashlrai/phantom-secrets)) | *Can this secret enter the model?* Real keys stay in the vault behind `phm_` placeholders; agents get value-blind metadata only. |
| Identity plane | `locus` (this repo) | *As whom, against which tenant, right now?* Every CLI command and MCP tool is sealed to one pinned binding. |

**Wire both into your client** — the pair is declared as `required_servers` on every `locus agent report`. `locus agent setup --apply` registers `locus`; [Phantom's own setup](https://github.com/ashlrai/phantom-secrets) registers `phantom`:

```json
{ "mcpServers": {
    "locus":   { "command": "locus", "args": ["mcp", "--supervised"] },
    "phantom": { "command": "phantom", "args": ["mcp", "serve"] }
} }
```

Do not register raw Supabase / Vercel / GitHub MCP servers with ambient tokens next to Locus — they bypass the pin.

**For orchestrators** (the [Phantom workbench](https://github.com/ashlrai/phantom), or your own hub): shell out to the CLI — do not reimplement pin or seal. `locus agent report --json` is the single entrypoint (exit 0 ready / 1 protected / 2 unsafe); `locus ci mint` hands each job an ephemeral sealed session; `locus mcp mint` issues one revocable tenant grant per job against the multi-tenant HTTP server. Drop-in TypeScript helpers and the full contract live in [integrations/phantom/](./integrations/phantom/) and [docs/hub-integration.md](./docs/hub-integration.md).

**On credentials:** bindings carry `credential_ref` pointers, never secrets. `env:VAR` resolves today — the operator supplies the value and Locus injects it only into the pinned provider's child environment. `phm:NAME` stays readable for metadata and migration, but resolution fails closed until a reviewed, tenant-scoped bridge exists; see [credential compatibility](./docs/credential-compatibility.md). Neither server's tool results ever contain a resolved secret value.

---

## Core concepts

| Term | Meaning |
|------|---------|
| **Binding** | principal × tenant × providers × CredentialRefs × policy |
| **Session** | Live pin sealed to exactly one Binding |
| **Workspace** | `.locus.toml` — default pin + allowlist for a repo tree |
| **CredentialRef** | `phm:NAME` / vault pointer — never the secret itself |

Workspace policy discovery is fail closed: if the nearest `.locus.toml` exists but is unreadable or malformed, explicit pin, autopin, and `--force` pin stop with an error, while `locus doctor` reports `UNSAFE`.

### Learned bindings: `locus autopin suggest`

locus keeps a local pin history (`$LOCUS_HOME/pin-history.jsonl`, 0600, remotes stored only as keyed hashes). In a repo, `locus autopin suggest` shows which binding you usually pin there, with pins, `pin_mismatch` refusals and last seen per alias. A suggestion needs a score (pins minus mismatches) of at least 3 and a strict lead; otherwise it suggests nothing. When the repo has an `origin` remote only `origin` is matched, so an extra remote that mimics another repo borrows nothing; remotes are not authenticated, though, so an `origin` that mimics another repo still matches its history. Check the binding and file `--apply` prints. A read-only `suggest` never creates `history.key`. Add `--json` for the `locus.autopin.suggest.v1` shape. `locus autopin suggest --apply` writes `default_binding` to the nearest `.locus.toml` inside the git toplevel (or a new one there), refuses a parent `.locus.toml` above the toplevel, and requires `LOCUS_CONTROL_CAPABILITY`, so an agent cannot apply it. Turn history off with `[history] enabled = false` or `LOCUS_PIN_HISTORY=0`.

Example binding (`~/.locus/bindings/acme.toml`):

```toml
[binding]
id = "bnd_acme"
alias = "acme"
tenant = "acme-corp"
description = "Acme client engagement"

[binding.policy]
default = "allow"
require_approval = ["*.delete*", "vercel.deploy.prod"]
dual_control = ["*.delete*", "vercel.deploy.prod"]  # two distinct principals
max_ttl = "8h"

[[binding.providers]]
provider = "supabase"
account = "acme-prod"
credential_ref = "env:LOCUS_SUPABASE_ACME"
scope = { project_ref = "abcdefghij", read_only = true }

[[binding.providers]]
provider = "github"
account = "acme-corp"
credential_ref = "env:LOCUS_GH_TOKEN_ACME"
scope = { orgs = ["acme-corp"], repos = ["acme-corp/*"] }
```

---

## CLI

```
locus init [--with-samples] · quickstart
locus onboard [--detect-only] [--yes] [--reset]  # guided refs-only tenant setup
locus client add                      # guided client binding (flags for every prompt)
locus pin [alias] [--force] [--client claude] [--ns a,b] [--ttl 2h]
locus enter <alias>                   # firm workflow pin
locus switch <alias>                  # one-shot leave + enter
locus leave · renew                   # clear pin; extend expiry (opt-in per binding)
locus whoami [--json]
locus status [--oneline] [--json]
locus exec -- <command> [args...]
locus exec --expect <alias> -- <cmd>  # refuse (exit 1, child never starts) unless <alias> is pinned
locus exec --for gh -- gh pr list    # inject only the pinned GitHub credential
locus run -b <alias> -- <command>     # one-shot; global pin unchanged
locus binding list|show|add|rm|migrate-credential-refs
locus capability status|persist|unpersist  # where control authority lives
locus autopin suggest [--json] [--apply]  # learned per-repo binding from pin history
locus workspace --default <alias> [--allow a,b] [--require-pin]
locus doctor [--json]                 # SAFE|WARN|UNSAFE (exit 0/1/2; advisory-only WARN exits 0)
locus doctor --fix [--apply]          # preview safe repairs; --apply makes them, backing up first
locus watch [--once] [--require-ok]   # session heartbeat each tick (NDJSON with --json)
locus dashboard · serve               # local identity UI + API (127.0.0.1)
locus forensics export                # shareable pack (no secrets)
locus verify session · claim --text "…"  # verification plane: session pack · claim score
locus approve list|grant|status|wait|deny  # require_approval / dual-control
locus notify status|on|off            # desktop banners OFF by default
locus events --last N · events export # audit tail / fleet pulse / OTLP
locus graph list|export|import        # encrypted binding graph share (no secrets)
locus ci mint|env|run                 # short-lived sealed sessions for pipelines
locus engagement init|close           # client engagement lifecycle (binding + workspace)
locus offboard <alias> [--apply]      # plan first; --apply removes the client's local records
locus upstream list|suggest           # built-in upstream MCP recipes
locus adapter list|verify|trust       # provider adapter catalog + signature trust
locus adapter search|install|uninstall · registry export|verify-manifest  # community + release adapters
locus hook zsh|bash|fish
locus setup --client claude|cursor|codex|grok|gemini|generic
locus mcp [--supervised]              # run the stdio MCP server; --supervised serves pinned provider tools
locus mcp mint|list|revoke            # multi-tenant grant tokens
locus agent report|setup|doctor       # AI-native readiness (hub JSON)
locus goal status                     # northstar progress from GOALS.md
locus completion zsh|bash|fish|elvish|powershell  # shell completions
locus version                         # build version (also: locus --version)
locus topic <name>                    # dashboard · forensics · serve · goal · verify · …
```

### Previewing mutations

Use `--dry-run` to preview supported mutations, for example:

```sh
locus init --dry-run --json
```

See [previewing mutations with `--dry-run`](#previewing-mutations-with---dry-run)
for the complete command list, exceptions, and limits.

**Agency kit:** [`examples/agency-starter/`](./examples/agency-starter/) — personal ↔ client A ↔ client B, dual-control, workspaces, offboarding. Guide: [docs/agency-starter.md](./docs/agency-starter.md).

**Northstar / hub:** [GOALS.md](./GOALS.md) · [docs/hub-integration.md](./docs/hub-integration.md) · [integrations/phantom/](./integrations/phantom/)

Approvals:

```bash
# After an agent hits require_approval (MCP returns appr_…)
locus approve list
locus approve grant appr_… --as alice  # advisory review label only
# A second local label still cannot satisfy dual_control:
locus approve grant appr_… --as bob    # authority remains 0/2
locus events --last 20 --op approval.advisory --json
# Provider execution stays blocked pending a closed external authorization envelope.
```

Graph share (bindings + workspace templates only — CredentialRefs, never secret values):

```bash
export LOCUS_GRAPH_PASSPHRASE='…'   # required for encrypt/decrypt
locus graph list
locus graph export --out team.locusgraph
locus graph import team.locusgraph
```

CI ephemeral pins (do not touch `active.json`):

```bash
locus ci mint -b acme --json          # short-lived sealed session
eval "$(locus ci env -b acme)"        # export LOCUS_SESSION_ID + env
locus ci run -b acme -- npm test      # mint → run → cleanup
```

Override home for tests/CI: `LOCUS_HOME=/tmp/locus-test locus …`

---

## Roadmap

| Phase | Status |
|-------|--------|
| **0** Daemon-less control plane, pin/whoami/exec, isolation tests | done |
| **1** `locus-mcp`, credential resolve, adapters, scope freeze, setup | done |
| **2** Firm UX, doctor pane, run/ns, notify, local `locus graph`, `locus ci` | done (0.1.1) |
| **2.x / 0.2–0.5** Dashboard, forensics, HTTP MCP, agent report, goal loop; 0.3 dogfood polish, TTL, client add, MCP session pin-anchoring; 0.4 multi-tenant multiplexor, Anthropic/OpenAI adapters, `locus switch`; 0.5 signed registry manifests, hub MT drop-in | **0.5.2 released; 0.6.0 candidate prepared** |
| **3** Remote binding graph sync, dual-control packs, ~~offboard~~ (done), SIEM export | next |
| **4** Adapter SDK, broader prebuilt platforms | later |

See [GOALS.md](./GOALS.md) (living milestones), [PLAN.md](./PLAN.md), and [DESIGN.md](./DESIGN.md).

---

## Development

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo test --workspace
cargo build --release
./target/release/locus --help

# Full shell e2e (pin, isolation, MCP freeze/approval, dual-control,
# doctor, events, enter/run, notify, graph, ci, heartbeat, dashboard health,
# forensics, goal status — feature-detected)
./scripts/e2e.sh

# Full e2e with a custom TMPDIR; also checks workspace cleanup on exit
./scripts/tests/e2e-tmpdir.sh

# README quickstart rehearsal: replays the extracted 30-second start in a
# throwaway HOME (--dry-run prints the plan without running it)
./scripts/check-readme-quickstart.sh

# README link check: every markdown link, image and anchor must resolve
# (--external also HEAD-requests remote URLs; loopback hosts skipped)
node scripts/check-readme-links.mjs
```

See [CONTRIBUTING.md](./CONTRIBUTING.md) for build/test/lint and how to add adapters. Agent-oriented rules: [AGENTS.md](./AGENTS.md). Short IDE guide: [CLAUDE.md](./CLAUDE.md).

### Building both binaries

The `locus-cli` package builds and installs both `locus` and `locus-mcp` in
one Cargo invocation, sharing `locus-core`. See [build verification](./docs/build-q8.md)
for the timing and artifact checks. The standalone `locus-mcp` package remains
available for server-only installs.

### Comparing exec launch latency

Open performance investigation [issue #170](https://github.com/ashlrai/locus/issues/170)
reports `locus exec -- true` as 3–5x slower than direnv and sops. That is the
issue's report, not a verified benchmark or a guarantee about current releases.
To investigate, run the complete
[same-host comparison](./docs/exec-latency.md#comparing-launchers-for-issue-170)
from the repository root in one Bash process on an idle Unix host. It builds
Locus in release mode, uses a fresh temporary home, a live synthetic pin and
disposable direnv and age/sops fixtures. No real credentials are needed.
Setup is outside the timed region; the four commands launch the same child:

```bash
/usr/bin/true
"$locus" exec --strict-creds -- /usr/bin/true
direnv exec "$work/direnv" /usr/bin/true
sops exec-env "$work/encrypted.env" /usr/bin/true
```

Each launcher supplies the same synthetic `RESEND_API_KEY` to the child; the
direct baseline supplies none. The procedure checks injection without printing
the value. It defines these paths and runs four discarded warmup rounds
followed by 32 measured rounds per command, rotating the starting order. It
aborts on any failed command and reports min, median and max wall time. Repeat
the whole procedure at least three times with fresh homes and record each run’s
min/median/max in milliseconds for all four commands, including the sample count.
Report the range of run medians for each command and compare within-run medians;
do not treat differences smaller than run-to-run variation as conclusive. Record
the commit, OS and architecture, filesystem, host load, build profile, and versions of Locus,
Rust, Cargo, Python, direnv, sops and age. For example, record `git rev-parse
HEAD`, `uname -srm`, `rustc --version`, `cargo --version`, `python3 --version`,
`direnv --version`, `sops --version`, `age-keygen --version`, and the built
`locus --version`; disable the sops version check as in the linked setup.

The historical comparison in [benchmarks](./docs/BENCHMARKS.md) used Linux
6.12 x86_64, 4 vCPU, 15.6 GiB RAM, rustc 1.99.1, Locus commit `e969d9c4`
(release build), direnv 2.38.2, sops 3.13.3 and age 1.3.2. It used one warmup
and 20 measured runs; Locus resolved no secrets and warned about missing sample
credentials. Its results are not the same experiment as the procedure above.
These launchers provide different guarantees: Locus verifies a pin, isolates the
child, resolves an environment reference and writes an audit record; direnv
loads an allowed `.envrc`; sops decrypts a synthetic dotenv file. The comparison
does not measure provider API calls or real secret-manager resolution. Timing
includes process startup and filesystem effects, but not terminal rendering.
Results depend on the host and conditions recorded above; they are observations,
not latency or relative-performance guarantees.

#### Run the comparison locally

Run this complete block from the repository root on an idle Unix host. It requires
Bash, Python 3, Rust/Cargo, `direnv`, `sops`, `age-keygen`, and `/usr/bin/true`.
Cargo may need network access for locked dependencies. The block builds the release CLI, creates a disposable home
and binding, and removes the temporary files on exit. It does not use real
credentials or change your existing pin. Keep the block in one Bash process so
its broker remains alive. `direnv allow` applies only to the temporary fixture.

```bash
bash <<'BASH'
set -euo pipefail
repo="$PWD"
while IFS= read -r name; do unset "$name"; done < <(compgen -A variable LOCUS_ || true)
while IFS= read -r name; do unset "$name"; done < <(compgen -A variable DIRENV_ || true)
while IFS= read -r name; do unset "$name"; done < <(compgen -A variable SOPS_ || true)
unset RESEND_API_KEY
export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
export PATH="$CARGO_HOME/bin:$PATH" CARGO_TARGET_DIR="$repo/target"
work="$(mktemp -d "${TMPDIR:-/tmp}/locus-peers.XXXXXXXX")"
work="$(cd "$work" && pwd -P)" # direnv allow and exec must use the same path
trap 'rm -rf "$work"' EXIT
export HOME="$work/home" LOCUS_HOME="$work/locus" DIRENV_LOG_FORMAT=
export SOPS_AGE_KEY_FILE="$work/age-key.txt" SOPS_DISABLE_VERSION_CHECK=1
mkdir -p "$HOME" "$work/direnv"
cargo build --locked --release -p locus-cli
locus="$repo/target/release/locus"
cd "$work"
"$locus" init
eval "$("$locus" hook bash)"
"$locus" binding add bench --tenant bench --provider resend --account bench \
  --credential-ref env:LOCUS_BENCH_FIXTURE --non-interactive
export LOCUS_BENCH_FIXTURE=fixture
"$locus" pin bench
printf 'export RESEND_API_KEY=fixture\n' > "$work/direnv/.envrc"
direnv allow "$work/direnv/.envrc"
printf 'RESEND_API_KEY=fixture\n' > "$work/plain.env"
age-keygen -o "$SOPS_AGE_KEY_FILE" 2>/dev/null
recipient="$(age-keygen -y "$SOPS_AGE_KEY_FILE")"
sops --encrypt --age "$recipient" --input-type dotenv --output-type dotenv \
  "$work/plain.env" > "$work/encrypted.env"
rm "$work/plain.env"
# Verify the synthetic value reaches each child without displaying it.
"$locus" exec --strict-creds -- /bin/sh -c 'test "$RESEND_API_KEY" = fixture'
direnv exec "$work/direnv" /bin/sh -c 'test "$RESEND_API_KEY" = fixture'
sops exec-env "$work/encrypted.env" 'test "$RESEND_API_KEY" = fixture'
python3 - "$locus" "$work/direnv" "$work/encrypted.env" <<'PYBENCH'
import statistics
import subprocess
import sys
import time

locus, directory, encrypted = sys.argv[1:]
commands = {
    "direct": ["/usr/bin/true"],
    "locus": [locus, "exec", "--strict-creds", "--", "/usr/bin/true"],
    "direnv": ["direnv", "exec", directory, "/usr/bin/true"],
    "sops": ["sops", "exec-env", encrypted, "/usr/bin/true"],
}
samples = {name: [] for name in commands}
names = list(commands)
warmups, runs = 4, 32
for i in range(warmups + runs):
    for name in names[i % len(names):] + names[:i % len(names)]:
        start = time.perf_counter_ns()
        result = subprocess.run(commands[name], stdin=subprocess.DEVNULL,
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        elapsed = (time.perf_counter_ns() - start) / 1_000_000
        if result.returncode != 0:
            raise SystemExit(f"{name} run {i + 1} failed: exit {result.returncode}; no results")
        if i >= warmups:
            samples[name].append(elapsed)
for name, values in samples.items():
    print(f"{name}: n={len(values)} min={min(values):.3f} "
          f"median={statistics.median(values):.3f} max={max(values):.3f} ms")
PYBENCH
"$locus" leave
BASH
```

All three launchers run `/usr/bin/true` with the same synthetic `RESEND_API_KEY`;
the direct baseline supplies none. Setup, pinning, encryption, and checks are
untimed. Four warmup rounds are discarded, followed by 32 measured rounds per
command, rotating the starting order.
The script aborts without statistics on a failed invocation and reports sample
count and minimum, median, and maximum wall time in milliseconds. Repeat the
whole block at least three times with fresh homes; record each result and the
range of run medians, along with the commit, OS and architecture, filesystem,
host load, release profile, and tool versions. Compare medians within each run
and account for run-to-run variation. The child workload and launcher-injected value are
the same, but the launchers perform different security and setup work as noted
above; these measurements do not establish equivalent isolation or predict
performance on another machine.

### Landing page & dashboard

| App | Role |
|-----|------|
| [`apps/web/`](./apps/web) | Marketing landing — AI-native identity plane hero |
| [`apps/dashboard/`](./apps/dashboard) | Operator UI embedded by `locus serve` / `dashboard` |

```bash
cd apps/web && npm start   # http://localhost:3000
locus dashboard            # http://127.0.0.1:8750 (needs built CLI)
```

Deploy notes (Vercel / Cloudflare Pages): [apps/web/README.md](./apps/web/README.md).

### Docs

| Doc | Topic |
|-----|--------|
| [AGENTS.md](./AGENTS.md) | AI coding agents: build, test, invariants, secrets |
| [CLAUDE.md](./CLAUDE.md) | Short development guide |
| [apps/web/](./apps/web) | Landing page (static HTML) |
| [apps/dashboard/](./apps/dashboard) | Local identity dashboard UI |
| [GOALS.md](./GOALS.md) | Northstar goal loop |
| [scripts/e2e.sh](./scripts/e2e.sh) | End-to-end shell suite |
| [scripts/check-readme-quickstart.sh](./scripts/check-readme-quickstart.sh) | README quickstart replay in a throwaway HOME |
| [scripts/check-readme-links.mjs](./scripts/check-readme-links.mjs) | README link / anchor / image resolution check (`--external` for remote) |
| [docs/README.md](./docs/README.md) | CLI dry-run support and limitations |
| [docs/architecture.md](./docs/architecture.md) | System diagram (DESIGN distilled) |
| [docs/BENCHMARKS.md](./docs/BENCHMARKS.md) | Install and core-op timings, peers, method |
| [docs/FIRST-RUN.md](./docs/FIRST-RUN.md) | First-run log and agent-use audit |
| [docs/agency-certainty.md](./docs/agency-certainty.md) | Identity vs epistemic certainty (Ashlr stack) |
| [docs/firm-mode.md](./docs/firm-mode.md) | Agencies: bindings, dual-control, workspaces |
| [docs/agency-starter.md](./docs/agency-starter.md) | Agency starter kit + doctor single pane |
| [examples/agency-starter/](./examples/agency-starter/) | Bindings, workspaces, dual-control, offboarding |
| [docs/phantom-enrolled-repos.md](./docs/phantom-enrolled-repos.md) | Phantom workbench: personal / company / client scope bindings per enrolled repo |
| [docs/hub-integration.md](./docs/hub-integration.md) | Phantom ↔ Locus contract: agent report, fleet gate, MCP grants |
| [integrations/phantom/](./integrations/phantom/) | Drop-in TS helpers + MCP gateway / doctor snippets |
| [docs/gateway-base-urls.md](./docs/gateway-base-urls.md) | Model gateways (for example a LiteLLM proxy): Locus makes no provider calls, `locus exec` drops unknown env vars, and how to set a gateway URL inside it |
| [docs/PQ-READINESS.md](./docs/PQ-READINESS.md) | Post-quantum readiness note: what Locus uses today (Ed25519 for adapter manifests, classical only) and what a migration would touch. No quantum-safety claim |
| [docs/mcp.md](./docs/mcp.md) | `locus-mcp` with Claude Code / Cursor |
| [docs/onboarding.md](./docs/onboarding.md) | Agency onboarding: 3 agent clients × 3 tenants end-to-end |
| [docs/adapters.md](./docs/adapters.md) | Writing a provider adapter |
| [docs/workers.md](./docs/workers.md) | Synthetic vs MCP stdio workers |
| [SECURITY.md](./SECURITY.md) | Threat model summary & reporting |
| [CHANGELOG.md](./CHANGELOG.md) | Release notes |
| [DESIGN.md](./DESIGN.md) / [PLAN.md](./PLAN.md) | Full architecture & roadmap |

---

## Daily firm workflow

```bash
locus engagement init acme --tenant acme-corp --workspace
locus enter acme
locus whoami
locus exec -- gh pr list
locus run -b personal -- npm test    # one-shot; global pin unchanged
locus leave

locus approve list
locus approve grant appr_… --as mason  # advisory only; never execution authority
locus notify status                  # OFF by default (no spam)
locus doctor                         # SAFE | WARN | UNSAFE

# Encrypted binding-graph share (CredentialRefs only — never secret values)
export LOCUS_GRAPH_PASSPHRASE='…'    # or interactive TTY prompt
locus graph export --out team.locusgraph
locus graph import team.locusgraph

# CI / ephemeral sealed pin (does not touch active.json)
locus ci mint -b acme --json         # session_id + env map; no secrets by default
```

**Desktop notifications are off by default.** Agents create many pending
approvals; banners only after `locus notify on` or `LOCUS_NOTIFY=1`
(silent, rate-limited). Kill switch: `locus notify off` / `LOCUS_QUIET=1`.

## Security model

- Session pins use **V3 HMAC seals plus a supervised live authority broker**. The broker binds the exact record digest, backing file, authority, expiry, and monotonic generation, so reading `daemon.key` alone cannot forge or replay current authority.
- Workspace `allowed_bindings` blocks wrong-tenant pins (unless `--force`, audited).
- `locus exec`, `locus run`, and `locus ci run` scrub ambient identity; resolved credentials are injected into the child by default. Their shared `--no-resolve` preflight expands recipe defaults and fails before child, worker, session, or credential effects when a declared upstream can resolve credentials. Credential-free upstreams remain usable.
- `locus exec --for <cli>` routes `gh`, `vercel`, `supabase`, or `aws` with only the selected provider's credential and fresh private configuration. See [CLI routing](docs/cli-routing.md) for credential formats and the trusted-process boundary.
- MCP never returns secret values; agents cannot pin (request only).
- Scope freeze: model cannot override frozen `project_ref` / `team_id`.
- Policy: globs + structured `[[rules]]`, `require_approval`, dual-control (2 principals).
- Upstream MCP workers start only after an authorized provider call. `tools/list` is discovery-only, multi-provider startup rolls back on partial failure, and each worker receives only its named provider's resolved credential keys.
- Drift freeze + hub heartbeat: `locus watch [--json] [--require-ok]` re-runs session verify each tick; doctor re-pin if binding changes under a session.

Details: [SECURITY.md](./SECURITY.md). Threat model: [DESIGN.md §9](./DESIGN.md).

---

## Part of the Ashlr ecosystem

Locus is one of the open-source tools from [Ashlr](https://ashlr.ai), a forward-deployed engineering company. Each one installs and runs on its own:

- **[Phantom](https://phm.dev)** (`phm`): the agent workbench. Source: [ashlrai/phantom](https://github.com/ashlrai/phantom).
- **[Phantom Secrets](https://phm.dev/secrets)** (`phantom`): keeps real API keys out of agent context with `phm_` placeholders. Source: [ashlrai/phantom-secrets](https://github.com/ashlrai/phantom-secrets).
- **[Lexicon](https://lexicon.ashlr.ai)** (`lexicon`): fixes the words dictation gets wrong.
- **[Ashlr MD](https://md.ashlr.ai)**: a Markdown app with an MCP server.
- **[Ashlr Stack](https://stack.ashlr.ai)** (`stack`): provisions services and stores their keys in Phantom Secrets.
- **[ashlr-plugin](https://plugin.ashlr.ai/)** (`ashlr-plugin`): token-efficiency tools for Claude Code and Codex.

## Contributing

Issues and PRs are welcome. Look for [`good first issue`](https://github.com/ashlrai/locus/labels/good%20first%20issue) to get started, and read [CONTRIBUTING.md](CONTRIBUTING.md). Report security problems privately per [SECURITY.md](SECURITY.md).

## License

MIT — see [LICENSE](./LICENSE).

Code of Conduct: [CODE_OF_CONDUCT.md](./CODE_OF_CONDUCT.md).
