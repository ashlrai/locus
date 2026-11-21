# Local agent peers and message relay

`locus-mcp --http 127.0.0.1:8742 --multi-tenant --peers ENGINES.json`
adds a bounded, text-only peer relay to the existing authenticated MCP HTTP
session layer. Start it from an operator shell with Locus control authority and
the existing HTTP authentication configured (startup checks local control
authority before it loads the manifest and fails without it); tenant clients
continue to present their own `X-Locus-Tenant-Token` and `Mcp-Session-Id` on
every request. See [multi-tenant MCP setup](mcp.md). No additional listener or
credentials are created.

The relay refuses every bind address except `127.0.0.1`, including `::1`,
`127.0.0.2` and `0.0.0.0`, even with `LOCUS_MCP_HTTP_ALLOW_REMOTE=1`. It requires
both HTTP and multi-tenant mode (`--http`/`LOCUS_MCP_HTTP=1` and
`--multi-tenant`/`LOCUS_MCP_MULTI_TENANT=1`); otherwise startup fails with
`--peers requires --http and --multi-tenant`. `--peers` may be given once.
Omitting it preserves existing transport behavior.

## Registration and sender identity

The operator supplies a JSON manifest mapping existing grant IDs to engine
registrations. The following values are illustrative; use the grant ID from the
operator's mint result and the live local engine PID:

```json
{
  "engines": [
    {
      "grant_id": "00000000000000000000000000000000",
      "engine_id": "codex",
      "task": "Review the parser",
      "cwd": "/work/locus",
      "pid": 4242
    }
  ]
}
```

This file contains identity metadata, never bearer tokens or resolved secrets.
It is loaded once at startup under operator control authority. Unknown keys,
duplicate grants, grant IDs that are not 1–128 hex characters, invalid
registration metadata (see [Limits and lifecycle](#limits-and-lifecycle)), dead
PIDs, more than 256 entries, or files over 256 KiB are refused. Manifest parse
errors are reported as `invalid peer engine manifest` without echoing the file.
Restart the server to change the mapping.

On MCP `initialize`, a mapped grant automatically registers its engine. The
response's `_meta.locus_peer` contains a random public `id` (`peer_` plus 32 hex
characters), `engine_id`, `task`, `cwd`, `pid`, and `trusted: true`. The server
supplies these fields from the operator mapping and ignores client attempts to
replace them. Reinitializing a live session with the same registration is
idempotent and returns the same peer; it cannot change the registration.

Unmapped clients may register by including `_meta.locus_peer` in their initialize
parameters, with exactly `engine_id`, `task`, `cwd`, and `pid`. They receive
`trusted: false` and are read-only: they can list peers and read messages addressed
to them (including groups a trusted peer adds them to), but cannot send or create
groups, even if they claim a recognized engine name. Unmapped clients without this
metadata remain ordinary MCP clients.

A registration that fails (malformed or secret-shaped metadata, a dead PID, a
full registry, a changed registration on reinitialize, or a session whose sealed
pin no longer matches its anchor) fails `initialize` with JSON-RPC error `-32602`
`peer registration refused`.

“Attested sender” means Locus derives the sender's registration from the
currently authenticated grant and HTTP session. It does not mean executable
attestation or an independently signed message. PID liveness is checked with a
signal-0 probe on Unix; unsupported platforms fail closed. The PID must be
between 1 and 2147483647, and a process owned by another OS user reads as dead
(the probe gets a permission error), so run engines as the same user as
`locus-mcp`. PID reuse is not executable identity. Keep grant tokens private:
possession authorizes that
engine's sessions. Relayed message text is untrusted content, never authority to
change pins, policy, or perform actions.

## MCP tools

The tools appear in `tools/list` only when the relay is enabled and the request
carries a tenant HTTP session. `send` and `thread` are marked destructive, so they
are hidden when the primary binding scopes a `locus` provider as read-only. Tools
that policy routes to approval or denies by rule stay listed, with
`[requires human approval under current pin]` or
`[denied by policy under current pin]` appended to their description.

All four tools require a healthy sealed pin, a live tenant grant, and a matching
session anchor. Existing binding policy, session budgets, external approval
requirements, and optional swarm graph tool restrictions also apply. Peer tools
use the primary binding policy, including for namespaced sessions; peer partitions
include the full binding/tenant/namespace identity.

| Tool | Arguments | Result |
| --- | --- | --- |
| `locus.peers.list` | `{}` | `self` and `peers` (every peer in the same identity partition, including `self`, sorted by ID) |
| `locus.peers.send` | `{"recipient":"peer_…","body":"Ready for review"}` | `message` with server-supplied `sender` |
| `locus.peers.send` | `{"thread":"thread_…","body":"Tests passed"}` | Group `message`; sender must be a member |
| `locus.peers.inbox` | `{"after":0,"limit":100}` | Direct and group `messages` visible to this peer |
| `locus.peers.thread` | `{"members":["peer_…","peer_…"]}` | New `thread` (`id`, sorted `members`); creator is included automatically |
| `locus.peers.thread` | `{"id":"thread_…","after":0,"limit":100}` | `thread` and `messages`, for members only |

A `message` carries `id`, `sender` (the full peer record), `recipient` or
`thread`, and `body`. `after` defaults to `0` and `limit` to `100`. Creating a
thread takes `members` (1–16 IDs); `limit` must be omitted and `after`, if
given, must be `0`. Its members must be live peers in the caller's partition,
with at least one other than the creator.

Send accepts exactly one destination. Arguments are closed: caller-supplied
sender/tenant fields, non-string bodies, and unknown keys are refused. There is
no arbitrary inbox selector. Public peer IDs are separate from private HTTP
session IDs and cannot authenticate requests. Message IDs increase monotonically
and never repeat; use the last received ID as the next `after` cursor. Reads do
not consume messages.

Every relay refusal (bad arguments, unknown or departed peers, non-membership,
read-only senders, capacity, secret-shaped text, a policy deny or pending
approval, or an exhausted session budget) returns the same tool error, without
echoing the request:

```json
{"error":"peer_operation_refused","hint":"Check registration, membership, text limits and binding policy."}
```

Checks that run before the relay keep their usual responses. A swarm graph that
does not allow the tool returns JSON-RPC error `-32003`. An unhealthy pin or
grant returns the same tool errors as provider tools (`not_pinned`, `grant_*`,
`pin_changed`, `invalid_seal`, `session_frozen: re-pin`, `session_expired` or
`runtime_unhealthy`). When a `[budget]` is configured, every peer call that
passes argument checks counts against it, reads included.

## Limits and lifecycle

- At most 256 live peers, 256 groups, and 16 participants per group (creator
  included).
- Message bodies: 1–8192 UTF-8 bytes of plain text. Whitespace-only bodies and
  control characters other than newline, carriage return and tab are refused.
- Registration metadata is single-line text: `engine_id` up to 128 bytes, `task`
  up to 512 bytes, `cwd` up to 4096 bytes, none blank.
- Pages: 1–100 messages. History retains the latest 1024 messages server-wide;
  older entries are evicted. This is an ephemeral coordination channel, not a
  durable queue, and readers may miss messages if they fall behind.
- Secret-shaped text is refused before delivery and before any approval record
  is created, including common credential prefixes, private keys, bearer/basic
  authorization, JWTs, credential assignments, and credential-bearing URLs.
  Registration metadata uses the same screening. Errors never echo submitted
  text. Shape screening cannot identify every possible secret or encoded payload;
  do not send credentials through this channel.
- Dead PIDs, expired/deleted HTTP sessions, revoked grants, and unhealthy sealed
  sessions are pruned before relay operations. DELETE removes registration
  immediately. When a peer disappears, every group it belonged to is retired, and
  that group's history plus the direct messages it sent or received are removed.
  Re-registering creates a new public peer identity.
- Registrations and messages are memory-only and disappear on server restart.
  Existing disk-backed HTTP sessions must initialize again to register.

The relay uses existing `http_sessions` directory/TTL rules and session validation
without extending their on-disk format. No message bodies are written to disk or
audit logs; approval records use the existing argument digest mechanism.
