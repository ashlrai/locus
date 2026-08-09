# ashlr-hub MCP gateway — Locus wiring

Exact changes so the hub discovers **only** the identity + secret planes, never ambient personal provider MCPs.

Contract: [`docs/hub-integration.md`](../../docs/hub-integration.md)  
Drop-in types: [`locus.ts`](./locus.ts)

---

## 1. REQUIRED_SERVERS (must)

Hub must treat Locus + Phantom as the agent safety pair. Do **not** auto-register raw Supabase / Vercel / GitHub MCPs for accounts that Locus pins.

```ts
// src/core/mcp/requiredServers.ts  (or equivalent)

/** Ashlr agent safety pair — identity plane + secret plane. */
export const REQUIRED_SERVERS = ["locus", "phantom"] as const;
export type RequiredServer = (typeof REQUIRED_SERVERS)[number];

/** Prefer report-driven list when available. */
export function requiredServersFromReport(
  report: { required_servers?: string[] } | null,
): readonly string[] {
  if (report?.required_servers?.length) return report.required_servers;
  return REQUIRED_SERVERS;
}
```

Also accept the constant exported from the drop-in:

```ts
import { REQUIRED_SERVERS } from "./integrations/locus";
// REQUIRED_SERVERS === ["locus", "phantom"]
```

Every `locus agent report --json` already emits:

```json
{
  "required_servers": ["locus", "phantom"],
  "mcp_command": "locus-mcp"
}
```

---

## 2. Server specs (spawn config)

```ts
// Prefer locusMcpServerSpecs() from integrations/ashlr-hub/locus.ts

const LOCUS_HOME = process.env.LOCUS_HOME ?? `${homedir()}/.locus`;

const mcpServers = {
  locus: {
    command: "locus-mcp",
    args: [] as string[],
    env: {
      LOCUS_HOME,
      LOCUS_NOTIFY: "0",
      LOCUS_CLIENT: "ashlr-hub",
      // Optional ephemeral pin (from locus ci mint):
      // LOCUS_SESSION_ID: mint.session_id,
    },
  },
  phantom: {
    command: "phantom-mcp", // or hub's existing phantom binary name
    args: [] as string[],
    env: {},
  },
};
```

**CI / job children:** after `locus ci mint -b <alias> --json`, pass `LOCUS_SESSION_ID` (and `LOCUS_HOME`) into the child. Use `withLocusSession(binding, fn)` from `locus.ts`.

---

## 3. Discovery filter (critical)

Hub already fans in every discovered MCP. That **increases** wrong-account risk if personal + client servers are both visible.

### Desired behavior

| Mode | Discovery |
|------|-----------|
| **Locus-first (recommended)** | Catalog = `REQUIRED_SERVERS` only (`locus` + `phantom`) |
| **Legacy / opt-out** | Full discovery, but **deny** registering provider servers that overlap pinned scopes |

### Pseudocode patch

```ts
// BEFORE (unsafe when multi-tenant):
// const servers = await discoverMcpServers(paths);

// AFTER — Locus-first:
import { REQUIRED_SERVERS, locusMcpServerSpecs } from "./integrations/locus";

async function resolveMcpCatalog(opts: {
  locusFirst?: boolean; // default true for firm/agent jobs
  paths?: string[];
}): Promise<Record<string, McpServerSpec>> {
  const locusFirst = opts.locusFirst !== false;

  if (locusFirst) {
    const base = locusMcpServerSpecs(process.env.LOCUS_HOME);
    // Merge phantom from hub ecosystem probe if not already present
    const discovered = await discoverMcpServers(opts.paths); // existing hub fn
    const phantom = discovered["phantom"] ?? discovered["phantom-secrets"];
    return {
      locus: base.locus,
      ...(phantom ? { phantom } : {}),
    };
  }

  // Legacy path: still strip ambient provider MCPs that Locus owns
  const discovered = await discoverMcpServers(opts.paths);
  const BLOCK = new Set([
    "supabase",
    "vercel",
    "github",
    "gh",
    "cloudflare",
    "aws",
    "stripe",
    "resend",
  ]);
  const out: Record<string, McpServerSpec> = {};
  for (const [name, spec] of Object.entries(discovered)) {
    if (BLOCK.has(name.toLowerCase())) continue;
    out[name] = spec;
  }
  // Always ensure required pair is present
  const required = locusMcpServerSpecs(process.env.LOCUS_HOME);
  out.locus = out.locus ?? required.locus;
  return out;
}
```

### Gateway tool naming

When hub multiplexes as `<server>__<tool>`:

- Control: `locus__locus_whoami`, `locus__locus_status`, …
- Provider tools appear only after a human/CI pin, still under the `locus` server prefix (or as bare tool names if hub flattens one server).

Do **not** re-expose `supabase__*` from a second process with ambient tokens.

---

## 4. Pre-job gate

```ts
import {
  ensureLocusReady,
  canMutate,
  locusAgentReport,
  withLocusSession,
} from "./integrations/locus";

// Interactive / long-lived agent (uses active pin or LOCUS_SESSION_ID):
ensureLocusReady(); // throws LocusNotReadyError if unsafe / unpinned

// Ephemeral hub job (does not touch human active.json):
await withLocusSession("acme", async ({ env }) => {
  ensureLocusReady(env);
  // spawn gateway children with env
});

// Soft check without throw:
const { report, gateOk } = locusAgentReport();
if (!gateOk || !canMutate(report!.status, report!.status_oneline)) {
  // surface next_steps to human; do not mutate
}
```

---

## 5. Checklist for the hub PR

- [ ] `REQUIRED_SERVERS = ["locus","phantom"]` exported and used by gateway bootstrap
- [ ] Default discovery path is **locus-first** (no ambient supabase/vercel/github MCPs)
- [ ] `LOCUS_SESSION_ID` forwarded for CI children from `ci mint` / `withLocusSession`
- [ ] Pre-mutate gate calls `ensureLocusReady` or equivalent
- [ ] Agent report consumed for status (`ready|protected|unsafe`) — never soft-allow `unsafe`
- [ ] Logs store alias/tenant and credential presence/source metadata only — never locators or resolved secrets
- [ ] Doctor includes `checkLocus` (see [doctor-check.md](./doctor-check.md))

---

## 6. What not to change

- Do not reimplement seal/pin in hub TypeScript.
- Do not let the model call `locus pin` — only `locus_request_pin` / human CLI.
- Do not set `LOCUS_NOTIFY=1` by default on hub-spawned MCP (noise; opt-in only).
