# Grant-scoped swarm broker

Current swarm execution backlog: [RQ-PLAN-LOCUS child jobs](swarm-child-jobs.md).

Use `locus serve --swarm /absolute/path/to/graph.json` to serve each swarm
agent only its explicitly granted MCP tools. Without `--swarm`, `locus serve`
continues to run the identity dashboard. The swarm endpoint is
`http://127.0.0.1:8750/mcp`; choose another loopback port with `--port`.

## Identity and authorization

The broker uses the existing [multi-tenant MCP transport](./mcp.md). Every
request needs the server bearer token and an operator-minted tenant grant in
`X-Locus-Tenant-Token`. MCP session IDs are bound to that grant and cannot be
reused by another grant.

The operator maps each tenant grant's public ID to one graph node. That node's
`id` is the agent ID. Headers, query parameters, tool arguments, and client
initialization metadata cannot select an agent or another tenant. Two agents
can share a tenant while receiving different tool lists. Each agent needs its
own tenant grant; sharing a grant shares its agent identity and authority.

The effective tool list is the intersection of the node's exact tool names and
the live tenant catalog. All existing seal, session authority, expiry,
revocation, anchor, binding drift, policy, scope freeze, and external approval
checks still apply. Listing never starts credential-bearing workers. A name in
the graph cannot create a tool or authorize an operation that its binding
rejects. Calls enforce the graph before dispatch, including calls to control
tools that would otherwise be universally available.

An authenticated grant absent from the graph gets **zero tools**, including no
`locus_*` fallback. An unhealthy backing session also gets zero tools. Invalid,
revoked, and expired bearer grants are rejected by the transport. Swarm mode
does not expose MCP resources or prompts. Transport health, grant-local
identity metadata, and session SSE ticks remain available to authenticated
grants; a zero-tool catalog does not conceal a grant's own identity metadata.

## Graph format

Keep this operator-controlled JSON outside agent-writable workspaces. It stores
public grant IDs and exact tool names, never bearer tokens or credentials.
Replace the illustrative grant IDs with IDs from your own grant inventory.

```json
{
  "version": 1,
  "nodes": [
    {
      "id": "graph/researcher",
      "grant_id": "0123456789abcdef",
      "tools": ["locus_whoami", "github.scope"]
    },
    {
      "id": "graph/reviewer",
      "grant_id": "fedcba9876543210",
      "tools": ["locus_heartbeat"]
    }
  ]
}
```

Duplicate node IDs, duplicate grant IDs, duplicate tool names, unknown fields,
unsupported versions, and wildcard grants are rejected. Empty graphs and empty
tool lists grant nothing. Limits are 1 MiB per manifest, 1,024 nodes, 1,024 tools
per node, 128 ASCII bytes per node ID, and 256 ASCII bytes per tool name. The
broker loads an immutable snapshot at startup. Restart it to apply graph
changes; deleting or editing the file does not revoke existing authority.
Revoke the tenant grant to block its next request. An in-flight call that
already passed the execution gate may complete.

## Operator launch

Before launch, configure bindings and mint a separate grant for each agent using
the [operator grant workflow](./mcp.md#multi-tenant-http---multi-tenant).
Minting and revocation change authority and require an authorized operator.
Deliver each returned bearer directly to its agent through your secret delivery
channel; never write it into this graph or logs.

From a long-lived operator shell with `LOCUS_CONTROL_CAPABILITY` available,
supply `LOCUS_MCP_HTTP_TOKEN` through your secret manager, then run:

```bash
locus serve --swarm /absolute/path/to/graph.json --port 8750
```

The CLI requires every configured graph grant to be live with a healthy sealed
session before launching. It uses the sibling `locus-mcp` binary or searches
`PATH`; `--mcp-bin /absolute/path/to/locus-mcp` selects an explicit binary. Use
matching CLI and MCP versions. `--open` and dashboard `--token` are incompatible
with swarm mode; unset `LOCUS_DASHBOARD_TOKEN` if it is present in the shell.

The supervisor clears the inherited environment, retaining runtime necessities,
the explicit Locus home, transport/operator capabilities, and only `env:`
credential locators referenced by the graph's validated bindings. It does not
mint grants, pin, or resolve credentials. Runtime, loader, proxy, CLI config,
and Locus authority/transport credential locator names are refused. The
`LOCUS_GH_TOKEN_*`, `LOCUS_SUPABASE_*`, and `LOCUS_VERCEL_TOKEN_*` sample
credential namespaces, `LOCUS_PROVIDER_TOKEN`, and `LOCUS_DEMO_UNUSED` remain
supported; other `LOCUS_*` locators are reserved. Provider workers still receive
only their selected binding's isolated environment. `phm:` resolution retains
its existing [fail-closed compatibility behavior](./credential-compatibility.md).

The underlying server can also be launched by an operator directly:

```bash
locus-mcp --http 127.0.0.1:8750 --multi-tenant --swarm /absolute/path/to/graph.json
```

This direct entry requires the same operator and HTTP capabilities. It uses the
environment supplied by its supervisor, so prefer `locus serve --swarm` for
environment narrowing. The graph option is rejected outside multi-tenant HTTP
mode. The swarm CLI always binds loopback; remote access should use an
authenticated transport managed by the operator.

## Audit and recovery

`swarm.serve` records supervisor start/exit with node count, port, and exit status.
`mcp.swarm.tools_list` records public grant ID, agent ID (null for an unmapped
grant), and the returned count. `mcp.swarm.tool_call` records the graph
authorization decision before dispatch. Its `allowed` field means the graph
and session gate passed; it does not assert execution succeeded. Existing MCP
provider execution audit events record downstream outcomes and include the public grant
ID for correlation.

These swarm events contain no arguments or bearer tokens. Denied tool names are
hashed rather than copied from untrusted input. If the audit append fails, the
broker refuses catalog delivery and calls; GET capabilities returns an empty
tool list. Restore audit storage before retrying. For a dead grant, have the
operator mint a replacement, update the graph's public grant ID, and restart
the broker. There is no active-pin fallback.
