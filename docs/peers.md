# Local peer registry

`locus peers` discovers cooperating agent sessions on this machine. It stores
only a public session label, engine ID, task description, absolute working
directory, and process ID. This is advisory metadata: it does not pin a binding,
authenticate an engine, grant provider access, or change sealed session authority.
Use Locus's existing pin and policy gates for tool access.

Start a registry in one terminal:

```sh
locus peers serve
```

Register a long-lived shell or agent session from another terminal:

```sh
locus peers register --session-id review-a --engine-id codex \
  --task 'Review parser tests' --cwd "$PWD" --pid "$$"
locus peers list --json
```

Use the actual long-lived agent PID when registering on an agent's behalf.
Do not use the PID of the short-lived registration command. Labels and task
descriptions must not contain secrets. Session labels here are public discovery
identifiers, not sealed-session IDs or bearer credentials.

The server defaults to `127.0.0.1:8751`. `serve --bind 127.0.0.1:PORT` and
`register/list --port PORT` select another port. Every other bind address is
refused, including `0.0.0.0`, `::1`, and other addresses in `127/8`. No environment
override enables remote access. Clients disable proxies and redirects.

The HTTP API is `GET /peers` and `POST /peers` with `application/json`:

```json
{
  "session_id": "review-a",
  "engine_id": "codex",
  "task": "Review parser tests",
  "cwd": "/work/locus",
  "pid": 12345
}
```

POST returns `{ "peer": ... }`; GET returns `{ "peers": [...] }`, sorted by
session label. Only `claude`, `cursor`, `codex`, `grok`, and `gemini` may register.
Unknown engines, including `generic`, have read-only discovery access; their
registration requests return HTTP 403. Engine names are caller-supplied labels,
not authenticated identities. All local native processes can read this registry.
Browser-origin requests and Host values other than the listener's numeric
loopback authority are refused. The API does not issue or require new tokens.

The registry is in memory and starts empty after a restart. Dead PIDs are pruned
before listing or accepting a registration. A live entry can refresh its task or
directory only with the same session label, engine, and PID; conflicts return
HTTP 409. OS liveness probes do not prove ownership and cannot distinguish PID
reuse; they are housekeeping, never authorization. A live process may stop
immediately after a probe. Uncertain OS liveness results retain entries.

The registry holds at most 256 entries, rejects oversized or malformed fields,
and caps HTTP request bodies at 32 KiB. Invalid or already-dead PIDs return HTTP
400; capacity exhaustion returns HTTP 503. `peers list --dry-run` is accepted;
`peers serve/register --dry-run` is rejected before any network operation.
