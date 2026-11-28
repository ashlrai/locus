import assert from "node:assert/strict";
import { readFileSync, existsSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

const dir = dirname(fileURLToPath(import.meta.url));
const root = resolve(dir, "../..");
const read = (path) => readFileSync(resolve(root, path), "utf8");
const readme = read("integrations/phantom/README.md");

test("verification instructions build and run the checkout's integration checks", () => {
  const section = readme.split("## Verification\n")[1]?.split("\n---")[0];
  assert.ok(section, "verification section exists");
  const commands = section.match(/```bash\n([\s\S]*?)```/)?.[1].trim().split("\n");
  assert.deepEqual(commands, [
    "cargo build -p locus-cli -p locus-mcp",
    "./scripts/hub-smoke.sh",
    "./scripts/hub-integration-test.sh",
  ]);
  for (const command of commands.slice(1)) assert.ok(existsSync(resolve(root, command)));
  for (const prerequisite of ["Cargo", "jq", "Node", "--experimental-strip-types", "curl"]) {
    assert.ok(section.includes(prerequisite), `missing ${prerequisite}`);
  }
  assert.match(section, /skip or failure as incomplete/);
});

test("prerequisites reflect the supported credential and session contracts", () => {
  const prerequisites = readme.split("## Prerequisites\n")[1]?.split("\n| Artifact")[0];
  assert.ok(prerequisites);
  assert.match(prerequisites, /`phm:NAME`[\s\S]*fail\s+closed/);
  assert.match(prerequisites, /complete validated `env`/);
  assert.match(prerequisites, /session ID by itself is not executor authority/);
  for (const path of ["docs/credential-compatibility.md", "docs/mcp.md", "integrations/phantom/fleet-preflight.md"]) {
    assert.ok(existsSync(resolve(root, path)), `${path} exists`);
  }
});

test("hub smoke prerequisites describe the script's local setup and exit contract", () => {
  const guide = read("docs/hub-integration-prerequisites.md");
  const script = read("scripts/hub-integration-test.sh");
  assert.match(read("docs/hub-integration.md"), /\]\(hub-integration-prerequisites\.md\)/);
  for (const term of ["jq", "Node", "--experimental-strip-types", "Cargo", "curl",
    "TMPDIR", "LOCUS_HOME", "LOCUS_SESSION_ID", "LOCUS_CONTROL_CAPABILITY",
    "HUB_INT_AMBIENT_CANARY", "127.0.0.1", "locus-mcp"]) {
    assert.ok(guide.includes(term), `guide missing ${term}`);
    assert.ok(script.includes(term === "Cargo" ? "cargo" : term === "Node" ? "node" : term),
      `script no longer references ${term}`);
  }
  assert.match(guide, /No running Phantom service[\s\S]*existing Locus pin is required/);
  assert.match(guide, /cargo build -p locus-cli -p locus-mcp\n\.\/scripts\/hub-integration-test\.sh/);
  assert.match(guide, /skip[\s\S]*does \*\*not\*\*[\s\S]*fail/);
  assert.match(script, /skip  multi-tenant MCP smoke/);
  assert.match(guide, /set -euo pipefail/);
  assert.match(script, /set -euo pipefail/);
  assert.match(guide, /== hub-integration-test OK ==/);
  assert.match(script, /== hub-integration-test OK ==/);
});
