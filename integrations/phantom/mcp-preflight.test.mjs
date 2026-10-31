import assert from "node:assert/strict";
import test from "node:test";
import * as phantom from "./locus.ts";
import * as legacyHub from "../ashlr-hub/locus.ts";

for (const [name, locus] of [["phantom", phantom], ["ashlr-hub", legacyHub]]) {
  function probe(body, status = 200) {
    const calls = [];
    return {
      calls,
      options: {
        baseUrl: "http://synthetic.invalid/",
        serverToken: "synthetic-server-token",
        headers: { [locus.TENANT_TOKEN_HEADER]: "synthetic-tenant-token" },
        fetchFn: async (url, init) => {
          calls.push({ url, init });
          if (url.endsWith("/health")) {
            return { ok: true, status: 200, json: async () => ({ ok: true }) };
          }
          return { ok: status === 200, status, json: async () => body };
        },
      },
    };
  }

  for (const mode of ["multi_tenant", "swarm"]) {
    test(`${name}: authenticated ${mode} is a grant-scoped multi-tenant server`, async () => {
      // Matches http_mcp_capabilities_tenant: swarm changes the mode label,
      // not the tenant grant contract. No real server or credentials are used.
      const p = probe({ ok: true, service: "locus-mcp", mode, grant_id: "gr_fixture" });
      assert.deepEqual(await locus.locusMtPreflight(p.options), {
        reachable: true, multiTenant: true, mode: "multi_tenant", status: 200,
        grantId: "gr_fixture",
      });
      assert.deepEqual(p.calls.map(c => c.url), [
        "http://synthetic.invalid/health", "http://synthetic.invalid/mcp",
      ]);
      assert.equal(p.calls[0].init.headers, undefined);
      assert.equal(p.calls[1].init.headers.Authorization, "Bearer synthetic-server-token");
      assert.equal(p.calls[1].init.headers[locus.TENANT_TOKEN_HEADER], "synthetic-tenant-token");
    });
  }

  test(`${name}: unauthenticated MT classification agrees with authenticated swarm`, async () => {
    const anonymous = probe({ error: "invalid_grant" }, 401);
    delete anonymous.options.headers[locus.TENANT_TOKEN_HEADER];
    const authenticated = probe({ mode: "swarm", grant_id: "gr_fixture" });
    const before = await locus.locusMtPreflight(anonymous.options);
    const after = await locus.locusMtPreflight(authenticated.options);
    assert.equal(before.multiTenant, true);
    assert.equal(before.mode, after.mode);
    assert.equal(before.multiTenant, after.multiTenant);
    assert.equal(before.auth.kind, "invalid_grant");
  });

  test(`${name}: ordinary single-tenant capabilities retain their classification`, async () => {
    const p = probe({ ok: true, service: "locus-mcp", capabilities: { tools: {} } });
    assert.deepEqual(await locus.locusMtPreflight(p.options), {
      reachable: true, multiTenant: false, mode: "single_tenant", status: 200,
    });
  });

  test(`${name}: server bearer failures are not evidence of multi-tenancy`, async () => {
    const p = probe({ error: "unauthorized" }, 401);
    const result = await locus.locusMtPreflight(p.options);
    assert.equal(result.multiTenant, false);
    assert.equal(result.mode, "unknown");
    assert.equal(result.auth.kind, "server_unauthorized");
  });
}
