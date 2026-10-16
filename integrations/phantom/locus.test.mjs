// Contract tests for the pure helpers in locus.ts. No spawn, no network, no FS.
// Run: node --experimental-strip-types --test integrations/phantom/locus.test.mjs
// Skips (does not fail) on a Node that cannot load TypeScript directly.
import test from "node:test";
import assert from "node:assert/strict";
import { fileURLToPath, pathToFileURL } from "node:url";

const target = pathToFileURL(fileURLToPath(new URL("./locus.ts", import.meta.url))).href;
let mod;
try {
  mod = await import(target);
} catch (error) {
  if (error && (error.code === "ERR_UNKNOWN_FILE_EXTENSION" || error.code === "ERR_UNSUPPORTED_TYPESCRIPT_SYNTAX")) {
    test("locus.ts contract tests", { skip: "Node cannot load TypeScript directly; use Node 22.6+ with --experimental-strip-types" }, () => {});
  } else {
    throw error;
  }
}

if (mod) {
  test("parseStatusOneline: pinned alias:tenant is healthy", () => {
    const parsed = mod.parseStatusOneline("personal:acme");
    assert.equal(parsed.kind, "pinned");
    assert.equal(parsed.alias, "personal");
    assert.equal(parsed.tenant, "acme");
    assert.equal(parsed.healthy, true);
  });

  test("parseStatusOneline: unpinned, frozen, require_pin, unknown tokens fail closed", () => {
    for (const raw of ["", "unpinned", "frozen", "require_pin", "invalid", "garbage", ":x", "x:"]) {
      assert.equal(mod.parseStatusOneline(raw).healthy, false, `raw=${JSON.stringify(raw)}`);
      assert.equal(mod.isStatusOnelineHealthy(raw), false);
    }
  });

  test("canMutate requires ready status and a healthy pin", () => {
    assert.equal(mod.canMutate("ready", "a:b"), true);
    assert.equal(mod.canMutate("ready", "unpinned"), false);
    assert.equal(mod.canMutate("unsafe", "a:b"), false);
    assert.equal(mod.canMutate("protected", "a:b"), false);
  });

  test("hasRequiredServers needs both locus and phantom, case-insensitively", () => {
    assert.deepEqual([...mod.REQUIRED_SERVERS], ["locus", "phantom"]);
    assert.equal(mod.hasRequiredServers(["locus", "phantom"]), true);
    assert.equal(mod.hasRequiredServers([" LOCUS ", "Phantom", "other"]), true);
    assert.equal(mod.hasRequiredServers(["locus"]), false);
    assert.equal(mod.hasRequiredServers([]), false);
    assert.equal(mod.hasRequiredServers(null), false);
  });

  test("parseLocusEnforceToken maps tokens to off, warn, enforce", () => {
    for (const v of [undefined, null, "", "0", "false", "no", "off"]) {
      assert.equal(mod.parseLocusEnforceToken(v), "off");
    }
    for (const v of ["warn", "log"]) assert.equal(mod.parseLocusEnforceToken(v), "warn");
    for (const v of ["1", "true", "yes", "enforce", "block", " ENFORCE "]) {
      assert.equal(mod.parseLocusEnforceToken(v), "enforce");
    }
  });

  test("resolveLocusEnforceMode: an explicitly set env value wins over config", () => {
    assert.equal(mod.resolveLocusEnforceMode({ LOCUS_ENFORCE: "off" }, { locus: { enforce: "enforce" } }), "off");
    assert.equal(mod.resolveLocusEnforceMode({ LOCUS_ENFORCE: "1" }, undefined), "enforce");
  });

  test("scrubbedChildEnv drops ambient credentials and the control capability", () => {
    const parent = {
      PATH: "/usr/bin",
      HOME: "/home/x",
      GH_TOKEN: "synthetic-ambient",
      SUPABASE_ACCESS_TOKEN: "synthetic-ambient",
      LOCUS_CONTROL_CAPABILITY: "synthetic-control",
    };
    const env = mod.scrubbedChildEnv(parent, { LOCUS_CONTROL_CAPABILITY: "synthetic-control-2", LOCUS_HOME: "/tmp/h" });
    assert.equal(env.PATH, "/usr/bin");
    assert.equal(env.HOME, "/home/x");
    assert.equal(env.LOCUS_HOME, "/tmp/h");
    assert.equal("GH_TOKEN" in env, false);
    assert.equal("SUPABASE_ACCESS_TOKEN" in env, false);
    assert.equal("LOCUS_CONTROL_CAPABILITY" in env, false);
  });

  test("validateMintEnv rejects non-objects, unknown keys, and credential-reference values", () => {
    for (const bad of [null, [], "x", 1]) assert.throws(() => mod.validateMintEnv(bad));
    assert.throws(() => mod.validateMintEnv({ GH_TOKEN: "x" }));
    assert.throws(() => mod.validateMintEnv({ LOCUS_GITHUB_ACCOUNT: "phm:SECRET" }));
    assert.throws(() => mod.validateMintEnv({ LOCUS_GITHUB_ACCOUNT: "env:TOKEN" }));
  });

  test("mergeLocusIntoMcpConfig adds locus without dropping existing servers and does not mutate input", () => {
    const input = { mcpServers: { phantom: { command: "phantom", args: ["mcp"] } } };
    const snapshot = JSON.stringify(input);
    const result = mod.mergeLocusIntoMcpConfig(input);
    assert.equal(JSON.stringify(input), snapshot);
    const text = JSON.stringify(result);
    assert.match(text, /"locus"/);
    assert.match(text, /"phantom"/);
  });
}
