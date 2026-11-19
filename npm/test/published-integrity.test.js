// Wrappers from 0.6.1 onward fail closed on checksum problems; 0.6.0 did not.
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const { join } = require("node:path");
const test = require("node:test");

const root = join(__dirname, "..", "..");
const read = (path) => readFileSync(join(root, path), "utf8");

test("fail-closed checksum behavior and 0.6.0 limits remain explicit", () => {
  for (const path of ["npm/README.md", "npm-mcp/README.md"]) {
    const text = read(path);
    assert.match(text, /Wrappers from v0\.6\.1 onward enforce the fail-closed/);
    assert.match(text, /Earlier v0\.6\.0 wrappers could install/);
    assert.match(text, /without verification when a checksum sidecar was missing or unparseable/);
    assert.match(text, /fall back to Cargo on a checksum mismatch/);
  }
  assert.match(read("npm/README.md"), /eager postinstall could also report success/);
  assert.match(read("README.md"), /Since v0\.6\.1 the npm wrappers fail closed/);
  assert.match(read("apps/web/public/index.html"), /Since v0\.6\.1 the npm wrappers fail closed/);
});
