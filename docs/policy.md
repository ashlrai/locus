# Policy & approvals

How Locus decides whether a tool call may proceed, and how humans grant (or deny) gated calls.

Identity is resolved at the gate; policy is the second gate: **before** any adapter or upstream worker runs.

## Binding policy surface

```toml
[binding.policy]
default = "allow"                 # allow | deny
max_ttl = "8h"
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
| `require_approval` | Legacy tool-name globs → single (or dual) human grant |
| `dual_control` | Legacy globs that always need two distinct principals |
| `dual_control_all_approvals` | Every `require_approval` match (rule or legacy) needs two principals |
| `default` | When nothing matches: `"allow"` or `"deny"` |

### Rule actions

| `action` | Effect |
|----------|--------|
| `allow` | Proceed (no approval) |
| `deny` | Hard deny (`denied_by_policy`) |
| `require_approval` | Block until human grant(s) |
| `dual_control` | Block until **two** distinct principals grant |
| anything else | Fail closed → deny |

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
   Disable anytime: `locus notify off` or `LOCUS_NOTIFY=0` / `LOCUS_QUIET=1`.
3. Human grants:
   ```bash
   locus approve list
   locus approve grant appr_… --as alice
   # dual_control: second principal
   locus approve grant appr_… --as bob
   ```
4. Scripts can block until the gate clears:
   ```bash
   locus approve wait appr_… --timeout 120
   ```
5. Agent re-calls with the **same args** (digest match) within the grant TTL (default 15m), or passes `confirm=true` + `approval_id`.

Deny:

```bash
locus approve deny appr_…
```

### CLI UX

| Command | Purpose |
|---------|---------|
| `locus approve list` / `pending` | Pending rows; shows **`grants 1/2`** for dual-control progress |
| `locus approve grant <id> [--as P] [--ttl 15m] [--touchid]` | Progress summary (n/required); `--touchid` = macOS confirm (fail closed) |
| `locus approve status <id>` | Full record + dual-control progress |
| `locus approve wait <id> [--timeout 120]` | Poll until approved / denied / timeout (exit non-zero on deny/timeout) |
| `locus approve deny <id>` | Terminal deny |

Principal default: `--as` → `LOCUS_PRINCIPAL` → `$USER`.

### Dual-control

Tools matching `action = "dual_control"`, legacy `dual_control` globs, or `require_approval` when `dual_control_all_approvals = true` need **two distinct** principals. Same principal cannot grant twice. One grant leaves `status=pending` with `grants 1/2`.

### Security notes

- Approval files store digests and labels only — never raw args or secrets.
- Approval ids are constrained (`[A-Za-z0-9_-]+`); path traversal is rejected.
- Prefer short TTLs for high-risk tools: `locus approve grant … --ttl 15m`.
- See [SECURITY.md](../SECURITY.md) and [docs/firm-mode.md](./firm-mode.md).

## Related

| Doc | Topic |
|-----|--------|
| [firm-mode.md](./firm-mode.md) | Multi-client agencies, dual-control ops |
| [architecture.md](./architecture.md) | System diagram |
| [DESIGN.md](../DESIGN.md) | Full design + threat model |
| [adapters.md](./adapters.md) | Mark destructive tools; scope freeze |
