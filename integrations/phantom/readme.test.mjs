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
