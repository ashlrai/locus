// The published 0.6.1 wrappers include the checksum hardening absent in 0.6.0.
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const { join } = require("node:path");
const test = require("node:test");

const root = join(__dirname, "..", "..");
const read = (path) => readFileSync(join(root, path), "utf8");
const published = JSON.parse(read("scripts/freshness/published.json")).version;

test("published 0.6.1 checksum behavior and 0.6.0 limits remain explicit", {
  skip: published !== "0.6.1" && "applies to the published 0.6.1 wrappers",
}, () => {
  for (const path of ["npm/README.md", "npm-mcp/README.md"]) {
    const text = read(path);
    assert.match(text, /published v0\.6\.1 wrapper enforces the fail-closed/);
    assert.match(text, /Earlier v0\.6\.0 wrappers could install/);
    assert.match(text, /without verification when a checksum sidecar was missing or unparseable/);
    assert.match(text, /fall back to Cargo on a checksum mismatch/);
  }
  assert.match(read("npm/README.md"), /eager postinstall could also report success/);
  assert.match(read("README.md"), /v0\.6\.1 npm wrappers fail closed/);
  assert.match(read("apps/web/public/index.html"), /v0\.6\.1 npm wrappers fail closed/);
});
