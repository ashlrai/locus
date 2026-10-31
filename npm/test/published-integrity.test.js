// The published 0.6.0 wrappers predate the source-side checksum hardening.
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const { join } = require("node:path");
const test = require("node:test");

const root = join(__dirname, "..", "..");
const read = (path) => readFileSync(join(root, path), "utf8");
const published = JSON.parse(read("scripts/freshness/published.json")).version;

test("published 0.6.0 checksum limits remain explicit", {
  skip: published !== "0.6.0" && "applies to the published 0.6.0 wrappers",
}, () => {
  for (const path of ["npm/README.md", "npm-mcp/README.md"]) {
    const text = read(path);
    assert.match(text, /Published v0\.6\.0 integrity limitation/);
    assert.match(text, /not in the\npublished v0\.6\.0 wrapper/);
    assert.match(text, /missing or unparseable\nchecksum sidecar can lead to an unverified installation/);
    assert.match(text, /checksum mismatch during first-use download can trigger a Cargo fallback/);
  }
  assert.match(read("npm/README.md"), /eager postinstall can also report success/);
  assert.match(read("README.md"), /Checksum fail-closed hardening is in post-v0\.6\.0 source,\nnot yet in a published npm release/);
  assert.match(read("apps/web/public/index.html"), /Checksum fail-closed hardening is in post-v0\.6\.0 source, not yet in a published npm release/);
});
