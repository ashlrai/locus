import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import * as phantom from "./locus.ts";
import * as legacyHub from "../ashlr-hub/locus.ts";

for (const [name, locus] of [["phantom", phantom], ["ashlr-hub", legacyHub]]) {
  const options = { locusHome: "/synthetic/locus-home", client: "config-test" };
  function configFile(t, raw) {
    const dir = mkdtempSync(join(tmpdir(), "locus-config-test-"));
    t.after(() => rmSync(dir, { recursive: true, force: true }));
    const path = join(dir, ".mcp.json");
    if (raw !== undefined) writeFileSync(path, raw, "utf8");
    return path;
  }

  for (const [shape, raw] of [
    ["truncated JSON", '{"mcpServers":{"other":{"command":"keep-me"}},'],
    ["empty file", ""],
    ["whitespace", " \n\t"],
    ["null root", "null"],
    ["array root", "[]"],
    ["string root", '"keep-me"'],
    ["null servers", '{"mcpServers":null,"keep":true}'],
    ["array servers", '{"mcpServers":[{"command":"keep-me"}]}'],
    ["scalar servers", '{"mcpServers":"keep-me"}'],
  ]) {
    test(`${name}: refuses ${shape} without altering the existing config`, (t) => {
      const path = configFile(t, raw);
      assert.throws(() => locus.registerLocusInMcpConfig(path, options), locus.LocusMcpConfigError);
      assert.equal(readFileSync(path, "utf8"), raw);
    });
  }

  test(`${name}: creates a missing config`, (t) => {
    const path = configFile(t);
    const result = locus.registerLocusInMcpConfig(path, options);
    assert.equal(result.created, true);
    assert.equal(result.written, true);
    assert.deepEqual(JSON.parse(readFileSync(path, "utf8")), result.config);
    assert.deepEqual(result.config.mcpServers.locus, locus.locusServerSpec(options));
  });

  test(`${name}: preserves unrelated config and is idempotent`, (t) => {
    const existing = { mcpServers: { other: { command: "other-server", args: ["--stdio"] } }, client: { theme: "dark" } };
    const path = configFile(t, JSON.stringify(existing));
    const result = locus.registerLocusInMcpConfig(path, options);
    assert.equal(result.created, false);
    assert.equal(result.written, true);
    assert.deepEqual(result.config.mcpServers.other, existing.mcpServers.other);
    assert.deepEqual(result.config.client, existing.client);
    const written = readFileSync(path, "utf8");
    const again = locus.registerLocusInMcpConfig(path, options);
    assert.equal(again.changed, false);
    assert.equal(again.written, false);
    assert.equal(readFileSync(path, "utf8"), written);
  });

  test(`${name}: accepts object config without mcpServers`, (t) => {
    const path = configFile(t, '{"keep":true}');
    const result = locus.registerLocusInMcpConfig(path, options);
    assert.equal(result.config.keep, true);
    assert.deepEqual(result.config.mcpServers.locus, locus.locusServerSpec(options));
  });

  test(`${name}: malformed input is not exposed in the error`, (t) => {
    const marker = "synthetic-private-config-marker";
    const path = configFile(t, `{"private":"${marker}",`);
    assert.throws(() => locus.registerLocusInMcpConfig(path, options), (error) => {
      assert.ok(error instanceof locus.LocusMcpConfigError);
      assert.ok(!error.message.includes(marker));
      return true;
    });
  });

  test(`${name}: forgiving pure parser retains its existing contract`, () => {
    assert.deepEqual(locus.parseMcpConfigJson("not-json"), { mcpServers: {} });
  });
}
