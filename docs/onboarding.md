# Onboarding — 3 clients × 3 tenants

The end-to-end walkthrough for an agency operator wiring **Codex CLI, Claude
Code, and Grok Build** across three tenants: **personal**, **company
(ashlr.ai)**, and a **client (cash-margin-partners)**. Every command below is
real; run them in order. Identity is resolved at the gate (`locus enter`), not
in the prompt — agents never pin and never see secrets.

## 0. Prerequisites

Install both binaries and put them on `PATH`:

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo install --path crates/locus-cli
cargo install --path crates/locus-mcp
which locus locus-mcp
```

(Or drop the release binaries `locus` + `locus-mcp` somewhere on `PATH`.)

Then initialize the store:

```bash
locus quickstart
```

`quickstart` (like `locus init`) **auto-mints the operator control
capability** (`LOCUS_CONTROL_CAPABILITY`) when none exists, persists it 0600
under `~/.locus/`, and adopts it for the current process. In every *new*
shell, load it without echoing the value:

```bash
eval "$(locus hook zsh)"    # or: bash | fish
```

Control commands (`init` / `quickstart` / `enter` / `pin` / `leave`) require
that capability; `locus-mcp` deliberately runs without it — agents never hold
control authority. `locus doctor` names the exact fix if it is missing,
invalid, or mismatched.

Secrets live in [Phantom](https://phm.dev) — bindings carry **CredentialRefs
only** (`phm:NAME`, or `env:VAR` as a fallback), never raw tokens. Before the
bindings below are usable, store the referenced tokens in Phantom under the
same names (`GH_PERSONAL`, `VERCEL_ASHLR`, `SUPABASE_CMP`).

## 1. Create the three tenant bindings

One binding per tenant. Alias is positional; everything else is flags:

```bash
# Personal GitHub
locus binding add personal \
  --tenant personal \
  --provider github \
  --account masonwyatt \
  --credential-ref phm:GH_PERSONAL

# Company — ashlr.ai on Vercel
locus binding add ashlr \
  --tenant ashlr.ai \
  --provider vercel \
  --account mason@ashlr.ai \
  --team-id team_ashlr \
  --credential-ref phm:VERCEL_ASHLR

# Client — Cash Margin Partners on Supabase (frozen to their project, read-only)
locus binding add cmp \
  --tenant cash-margin-partners \
  --provider supabase \
  --account cmp-ops \
  --project-ref <CMP_PROJECT_REF> \
  --credential-ref phm:SUPABASE_CMP \
  --read-only
```

`--project-ref` / `--team-id` / `--org` become **frozen scopes**: agent calls
that try a different selector are denied, not redirected.

Confirm (the active pin, once you have one, is marked in the list):

```bash
locus binding list
locus binding show cmp
```

Alternative for client tenants: `locus engagement init cmp --tenant
cash-margin-partners --workspace` creates the binding (phm: stub only) plus a
workspace file and `.locus/README.md` in one step.

## 2. Pin project directories to tenants

In each project directory, write a `.locus.toml` so the right tenant is the
only one reachable there:

```bash
cd ~/work/cmp-project
locus workspace --default cmp --allow cmp --require-pin
```

Repeat with `--default ashlr --allow ashlr` and `--default personal --allow
personal` in the other trees. This is what MCP auto-pin and `locus enter`
(with no alias) resolve against.

## 3. Wire the three agent clients

Run from the project directory (it writes/merges MCP config where the client
expects it and sets `LOCUS_AUTO_PIN=cwd` + `LOCUS_CLIENT=<client>`):

### Claude Code and Codex CLI (write + verify)

```bash
locus agent setup --dry-run --client claude    # preview
locus agent setup --apply   --client claude
locus agent setup --apply   --client codex
```

`--apply` re-reads each config after writing and **fails naming the
client + path** if the registration did not stick. Add `--workspace` to also
write a `.locus.toml` stub, and `--mcp-bin /abs/path/locus-mcp` if the GUI
launcher has a minimal `PATH`. Restart the client so the tool catalog
reloads.

### Grok Build

Grok Build's documented MCP config is `~/.grok/config.toml` (Codex-style
`[mcp_servers.<name>]` TOML). Locus writes it with the same fail-closed merge
used for Codex — an unparseable file aborts untouched, and only the `locus`
entry is upserted:

```bash
locus setup --client grok
locus agent setup --apply --client grok   # grok is included in --client all
```

Restart Grok Build (or use `/mcps` in its TUI) after changes; `grok mcp list`
shows the loaded server. The readiness probe (`mcp_registered.grok`) reads
`~/.grok/config.toml` by default; for a nonstandard location, override it:

```bash
export LOCUS_GROK_MCP_CONFIG=/path/to/config   # JSON mcpServers or TOML [mcp_servers]
```

For clients with no known config path at all, `locus setup --client generic`
prints a paste-ready stdio server entry (JSON and TOML) and writes nothing.

### Verify all three

```bash
locus doctor                 # store, capability, pin, workspace, phm: probes
locus agent doctor           # human-readable readiness (exit: ready=0, protected=1, unsafe=2)
locus agent report --json    # hub contract: ready / pin / mcp_registered / doctor
```

## 4. Daily rhythm

```bash
cd ~/work/cmp-project
locus enter            # resolves the workspace default (cmp); or: locus enter ashlr
locus whoami           # confirm tenant + providers + frozen scopes before any work
# ... agents work through locus-mcp ...
locus leave            # clear identity when done
```

Switching tenants is `locus leave` (or just `locus enter <other-alias>`) —
the MCP catalog follows the new pin on the next `tools/list`; restart the
client if it caches tools. `locus status --oneline` gives a prompt-friendly
`alias:tenant` marker, and `locus run -b personal -- <cmd>` runs one command
under a temporary pin without touching the global one.

What the agent sees vs. cannot do:

| Surface | Behavior |
|---------|----------|
| Unpinned | Control tools only (`locus_whoami`, `locus_safe_next`, `locus_request_pin`, …) — no ambient fallthrough to personal accounts |
| Pinned | Control tools + provider tools for **that binding only**; wrong-tenant tools do not appear |
| Pinning | Agents **cannot** pin; `locus_request_pin` returns instructions for the human |
| Scope | Frozen selectors (`project_ref`, `team_id`, `org`) reject alternates |
| Secrets | Never in MCP responses — credentials resolve into worker env only |

## 5. Troubleshooting

| Symptom | Fix |
|---------|-----|
| Wrong tenant pinned | `locus whoami`, then `locus leave` and `locus enter <alias>`; workspace `allowed_bindings` blocks out-of-scope aliases unless `--force` |
| Session frozen (binding drift) | Binding changed under a live pin — doctor/watch freezes it, fail closed. `locus doctor`, then `locus leave` + `locus enter <alias>` to re-seal |
| Stale / missing client config | Re-run `locus agent setup --apply --client <client>` (post-write verification names the failing client + path) or `locus setup --client <client>`; restart the client |
| Capability missing / mismatched | `eval "$(locus hook zsh)"` in this shell; `locus doctor` prints the exact fix; `locus quickstart` mints one only when none exists |
| Agent only sees `locus_*` tools | No valid pin — enter/pin first; seal may be expired or tampered (fail closed) |
| `binding not found` | Typo — the error lists known aliases with a nearest-match suggestion |
| `mcp_registered.grok` is `false` | Re-run `locus agent setup --apply --client grok` (writes `~/.grok/config.toml`); for a nonstandard path set `LOCUS_GROK_MCP_CONFIG` |
| `phm:` refs unresolved | Phantom missing from `PATH` or the named secret is absent — `locus doctor` probes both |

## Related

- [docs/mcp.md](./mcp.md) — client wiring details, auto-pin signals, HTTP transport
- [docs/firm-mode.md](./firm-mode.md) — dual-control, multi-client operations
- [docs/agency-starter.md](./agency-starter.md) — starter kit + doctor single pane
- [docs/policy.md](./policy.md) — allow / deny / require_approval
