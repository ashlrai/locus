# Policy & approvals

How Locus decides whether a tool call may proceed, and how humans grant (or deny) gated calls.

Identity is resolved at the gate; policy is the second gate: **before** any adapter or upstream worker runs.

## Binding policy surface

```toml
[binding.policy]
default = "allow"                 # allow | deny
max_ttl = "8h"                    # hard cap on any pin (default 8h)
default_ttl = "2h"                # optional: pin length when no --ttl is passed
parallel_sessions = 4

# Preferred: ordered structured rules (first match wins)
[[binding.policy.rules]]
match = "supabase.*"
action = "allow"

[[binding.policy.rules]]
match = "*.delete*"
action = "require_approval"

[[binding.policy.rules]]
match = "vercel.deploy.prod"
action = "dual_control"

# Legacy globs (still supported; evaluated after rules)
require_approval = ["*.drop*"]
dual_control = []
# dual_control_all_approvals = true   # every require_approval gate needs 2 principals
```

| Field | Role |
|-------|------|
| `rules` | Ordered list of `{ match, action }`. **First matching rule wins.** |
| `require_approval` | Legacy tool-name globs → fail-closed external authority gate |
| `dual_control` | Legacy globs that require two externally authenticated identities |
| `dual_control_all_approvals` | Every `require_approval` match (rule or legacy) needs two externally authenticated identities |
| `default` | When nothing matches: `"allow"` or `"deny"` |

### Rule actions

| `action` | Effect |
|----------|--------|
| `allow` | Proceed (no approval) |
| `deny` | Hard deny (`denied_by_policy`) |
| `require_approval` | Block until independently authenticated external authority exists |
| `dual_control` | Block until **two** distinct externally authenticated identities approve |
| anything else | Fail closed → deny |

### Session TTL (auto-leave)

Every pin expires; expiry is enforced passively and fail-closed by seal
verification on every privileged op (MCP gate, `exec`, `whoami`, `doctor`) —
there is no timer to kill. `locus enter <alias> --ttl 30m` requests a shorter
pin (min `1m`, max `24h`).

Precedence (winner always capped by `max_ttl`):

| Priority | Source | Example |
|----------|--------|---------|
| 1 | `--ttl` flag on `enter` / `pin` (or CI mint TTL) | `locus enter cmp --ttl 30m` |
| 2 | `policy.default_ttl` in the binding TOML | `default_ttl = "2h"` |
| 3 | `policy.max_ttl` (default `8h`) | `max_ttl = "8h"` |

A request above `max_ttl` is silently clamped (the CLI prints a cap warning).
An invalid `default_ttl` fails closed at binding save/load. Editing
`default_ttl` never freezes a live session — it is deliberately outside the
binding fingerprint. Remaining time surfaces in `whoami`
(`expires_in_secs`), `binding list` (`* pinned (… left)`), and `doctor`
emits a `pin_expiring` **Warn** finding in the final 5 minutes.

### Evaluation order

```
1. Structured rules     first matching [[binding.policy.rules]] entry
2. Legacy globs         require_approval, then dual_control
3. policy.default       allow | deny
```

Example: allow Supabase list tools, gate deletes, dual-control prod deploy:

```toml
[binding.policy]
default = "allow"

[[binding.policy.rules]]
match = "supabase.list*"
action = "allow"

[[binding.policy.rules]]
match = "*.delete*"
action = "require_approval"

[[binding.policy.rules]]
match = "vercel.deploy.prod"
action = "dual_control"
```

Because rules are ordered, a more-specific `allow` listed **above** a broader `require_approval` wins for that tool.

### Glob syntax

- `*` — any characters (greedy)
- `?` — single character
- Case-sensitive on the tool name (`supabase.table.delete`, `vercel.deploy.prod`)

## Approval flow

1. Agent hits a gated tool → Locus writes `$LOCUS_HOME/approvals/appr_….json` with `status=pending` and an `args_digest` (raw args **never** stored).
2. Desktop banners are **off by default** (agent sessions create many pending
   approvals and would spam Notification Center). Opt in with:

   ```bash
   locus notify on          # persists [notify] enabled = true
   export LOCUS_NOTIFY=1    # session override
   ```

   Banners are silent (no sound) and rate-limited to one per tool+binding per 60s.
   On the first dual-control advisory label, an opt-in banner reports the local
   review activity and says external broker authority is still required
   (separate rate limit per approval id).
   Disable anytime: `locus notify off` or `LOCUS_NOTIFY=0` / `LOCUS_QUIET=1`.
3. A local operator may record advisory evidence:
   ```bash
   locus approve list
   locus approve grant appr_… --as alice
   # another unverified local label (still not dual-control authority)
   locus approve grant appr_… --as bob
   ```
4. The record remains pending and provider execution remains blocked. `approve wait` cannot succeed without a future external authenticated envelope.

Deny:

```bash
locus approve deny appr_…
```

### CLI UX

| Command | Purpose |
|---------|---------|
| `locus approve list` | Pending rows; shows authoritative `0/N` and advisory assertion count |
| `locus approve grant <id> [--as P] [--touchid]` | Records an untrusted local advisory label only; confirmation is not identity authority |
| `locus approve status <id>` | Full record with explicit trust and disabled authority state |
| `locus approve wait <id> [--timeout 120]` | Poll until approved / denied / timeout (exit non-zero on deny/timeout) |
| `locus approve deny <id>` | Terminal deny |

Label default: `--as` → `LOCUS_PRINCIPAL` → `$USER`. These are caller-controlled strings, not authenticated identities.

### Dual-control

Tools matching `action = "dual_control"`, legacy `dual_control` globs, or `require_approval` when `dual_control_all_approvals = true` need two distinct externally authenticated identities. Local labels never count, so authority remains `0/2`.

### Security notes

- Approval files store digests and labels only — never raw args or secrets.
- Approval ids are constrained (`[A-Za-z0-9_-]+`); path traversal is rejected.
- External authority is not implemented; all gated provider calls fail closed.
- See [SECURITY.md](../SECURITY.md) and [docs/firm-mode.md](./firm-mode.md).

## Related

| Doc | Topic |
|-----|--------|
| [firm-mode.md](./firm-mode.md) | Multi-client agencies, dual-control ops |
| [architecture.md](./architecture.md) | System diagram |
| [DESIGN.md](../DESIGN.md) | Full design + threat model |
| [adapters.md](./adapters.md) | Mark destructive tools; scope freeze |
