// Keep the upgrade note aligned with the distinct CLI and MCP regressions in
// no-downgrade.test.js and mcp-no-downgrade.test.js.
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const { join } = require("node:path");
const test = require("node:test");

test("unreleased changelog distinguishes npm wrapper upgrade behavior", () => {
  const changelog = readFileSync(join(__dirname, "../..", "CHANGELOG.md"), "utf8");
  const unreleased = changelog.split("## [Unreleased]")[1].split("\n## [")[0];
  const note = unreleased.split("**npm wrapper upgrades")[1].split("\n- **")[0]
    .replace(/\s+/g, " ");

  assert.match(note, /equal cached version is reused; an older one can be upgraded/);
  assert.match(note, /newer or unversioned cache, `locus-cli` preserves the binary and refuses to run before checking PATH/);
  assert.match(note, /`@ashlrai\/locus-mcp` preserves and runs it/);
  assert.match(note, /leading wrapper `--force` permits replacing a protected cache/);
  assert.match(note, /published v0\.5\.2 CLI wrapper has the different behavior/);
});
