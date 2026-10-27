# Local agent peers and message relay

`locus-mcp --http 127.0.0.1:8742 --multi-tenant --peers ENGINES.json`
adds a bounded, text-only peer relay to the existing authenticated MCP HTTP
session layer. Start it from an operator shell with Locus control authority and
the existing HTTP authentication configured; tenant clients continue to present
their own `X-Locus-Tenant-Token` and `Mcp-Session-Id` on every request. See
[multi-tenant MCP setup](mcp.md). No additional listener or credentials are created.

The relay refuses every bind address except `127.0.0.1`, including `::1` and
`0.0.0.0`, even with `LOCUS_MCP_HTTP_ALLOW_REMOTE=1`. It requires both HTTP and
multi-tenant mode. Omitting `--peers` preserves existing transport behavior.

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
It is loaded once at startup under operator control authority. Duplicate grants,
invalid metadata, dead PIDs, more than 256 entries, or files over 256 KiB are
refused. Restart the server to change the mapping.

On MCP `initialize`, a mapped grant automatically registers its engine. The
response's `_meta.locus_peer` contains a random public `id`, `engine_id`, `task`,
`cwd`, `pid`, and `trusted: true`. The server supplies these fields from the
operator mapping and ignores client attempts to replace them. Reinitializing a
live session is idempotent and cannot change its registration.

Unmapped clients may register by including `_meta.locus_peer` in their initialize
parameters, with exactly `engine_id`, `task`, `cwd`, and `pid`. They receive
`trusted: false` and are read-only: they can list peers and read messages addressed
to them, but cannot send or create groups, even if they claim a recognized engine
name. Unmapped clients without this metadata remain ordinary MCP clients.

“Attested sender” means Locus derives the sender's registration from the
currently authenticated grant and HTTP session. It does not mean executable
attestation or an independently signed message. PID liveness is checked with an
OS process probe on Unix; unsupported platforms fail closed. PID reuse is not
executable identity. Keep grant tokens private: possession authorizes that
engine's sessions. Relayed message text is untrusted content, never authority to
change pins, policy, or perform actions.

## MCP tools

All four tools require a healthy sealed pin, a live tenant grant, and a matching
session anchor. Existing binding policy, session budgets, external approval
requirements, and optional swarm graph tool restrictions also apply. Peer tools
use the primary binding policy, including for namespaced sessions; peer partitions
include the full binding/tenant/namespace identity.

| Tool | Arguments | Result |
| --- | --- | --- |
| `locus.peers.list` | `{}` | `self` and `peers` in the same identity partition |
| `locus.peers.send` | `{"recipient":"peer_…","body":"Ready for review"}` | Message with server-supplied sender |
| `locus.peers.send` | `{"thread":"thread_…","body":"Tests passed"}` | Group message; sender must be a member |
| `locus.peers.inbox` | `{"after":0,"limit":100}` | Direct and group `messages` visible to this peer |
| `locus.peers.thread` | `{"members":["peer_…","peer_…"]}` | New `thread`; creator is included automatically |
| `locus.peers.thread` | `{"id":"thread_…","after":0,"limit":100}` | Thread and messages, for members only |

Send accepts exactly one destination. Arguments are closed: caller-supplied
sender/tenant fields, non-string bodies, and unknown keys are refused. There is
no arbitrary inbox selector. Public peer IDs are separate from private HTTP
session IDs and cannot authenticate requests. Message IDs increase monotonically;
use the last received ID as the next `after` cursor. Reads do not consume messages.

## Limits and lifecycle

- At most 256 live peers, 256 groups, and 16 participants per group.
- Message bodies: 1–8192 UTF-8 bytes, plain text only; metadata has separate caps.
- Pages: 1–100 messages. History retains the latest 1024 messages server-wide;
  older entries are evicted. This is an ephemeral coordination channel, not a
  durable queue, and readers may miss messages if they fall behind.
- Secret-shaped text is refused before delivery, including common credential
  prefixes, private keys, bearer/basic authorization, JWTs, credential assignments,
  and credential-bearing URLs. Registration metadata uses the same screening.
  Errors never echo submitted text. Shape screening cannot identify every possible
  secret or encoded payload; do not send credentials through this channel.
- Dead PIDs, expired/deleted HTTP sessions, revoked grants, and unhealthy sealed
  sessions are pruned before relay operations. DELETE removes registration
  immediately. A group is retired when any participant disappears, and associated
  history is removed. Re-registering creates a new public peer identity.
- Registrations and messages are memory-only and disappear on server restart.
  Existing disk-backed HTTP sessions must initialize again to register.

The relay uses existing `http_sessions` directory/TTL rules and session validation
without extending their on-disk format. No message bodies are written to disk or
audit logs; approval records use the existing argument digest mechanism.
