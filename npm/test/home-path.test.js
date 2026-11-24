const { test } = require("node:test");
const assert = require("node:assert/strict");
const { spawnSync } = require("node:child_process");
const { mkdtempSync, mkdirSync, rmSync, writeFileSync } = require("node:fs");
const { tmpdir } = require("node:os");
const { join } = require("node:path");

const root = join(__dirname, "..", "..");

function probe(packageDir, body) {
  const result = spawnSync(process.execPath, ["-e", body], {
    cwd: join(root, packageDir),
    encoding: "utf8",
  });
  assert.equal(result.status, 0, result.stderr);
  return result.stdout.trim().split(/\n/);
}

test("USERPROFILE is the cache home when HOME is empty", () => {
  const profile = mkdtempSync(join(tmpdir(), "locus-home-"));
  try {
    for (const [dir, file, binary] of [
      ["npm", "./bin/locus.js", "locus"],
      ["npm-mcp", "./bin/locus-mcp.js", "locus-mcp"],
    ]) {
      const lines = probe(dir, `
        process.env.HOME = "";
        process.env.USERPROFILE = ${JSON.stringify(profile)};
        const wrapper = require(${JSON.stringify(file)});
        console.log(wrapper.userHome());
        console.log(wrapper.getBinaryPath());
      `);
      const name = process.platform === "win32" ? `${binary}.exe` : binary;
      assert.equal(lines[0], profile);
      assert.equal(lines[1], join(profile, ".locus", "bin", name));
    }
  } finally {
    rmSync(profile, { recursive: true, force: true });
  }
});

test("a missing HOME and USERPROFILE does not use a hard-coded /tmp", () => {
  const lines = probe("npm", `
    delete process.env.HOME;
    delete process.env.USERPROFILE;
    const wrapper = require("./bin/locus.js");
    console.log(wrapper.userHome());
  `);
  assert.notEqual(lines[0], "/tmp");
  assert.ok(lines[0].length > 0);
});

test("a CRLF version sidecar still parses", () => {
  const dir = mkdtempSync(join(tmpdir(), "locus-ver-"));
  const binary = join(dir, "locus-not-a-shim");
  try {
    mkdirSync(dir, { recursive: true });
    writeFileSync(binary, "not an executable");
    writeFileSync(`${binary}.version`, "0.7.2\r\n");
    const lines = probe("npm", `
      const wrapper = require("./bin/locus.js");
      console.log(wrapper.readBinaryVersion(${JSON.stringify(binary)}));
    `);
    assert.equal(lines[0], "0.7.2");
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
