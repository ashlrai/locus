const { test } = require("node:test");
const assert = require("node:assert/strict");
const path = require("node:path");
const { spawnSync } = require("node:child_process");

const home = "C:\\Users\\dev";

function probe(packageDir, file) {
  const result = spawnSync(process.execPath, ["-e", `
    process.env.HOME = "";
    process.env.USERPROFILE = ${JSON.stringify(home)};
    const wrapper = require(${JSON.stringify(file)});
    console.log(wrapper.userHome());
  `], {
    cwd: packageDir,
    encoding: "utf8",
  });
  assert.equal(result.status, 0, result.stderr);
  return result.stdout.trim();
}

test("win32 joins the cache and cargo bins with backslashes", () => {
  const locus = require("../bin/locus.js");
  const mcp = require("../../npm-mcp/bin/locus-mcp.js");
  for (const wrapper of [locus, mcp]) {
    const cache = wrapper.cacheBinaryPath(home, "locus.exe", path.win32);
    const cargo = wrapper.cargoBinaryPath(home, "cargo.exe", path.win32);
    assert.equal(cache, "C:\\Users\\dev\\.locus\\bin\\locus.exe");
    assert.equal(cargo, "C:\\Users\\dev\\.cargo\\bin\\cargo.exe");
    assert.equal(cache.includes("/"), false);
    assert.equal(cargo.includes("/"), false);
    assert.equal(path.win32.isAbsolute(cache), true);
    assert.equal(path.win32.delimiter, ";");
  }
  assert.equal(
    mcp.cacheBinaryPath(home, "locus-mcp.exe", path.win32),
    "C:\\Users\\dev\\.locus\\bin\\locus-mcp.exe",
  );
});

test("an empty HOME keeps a Windows USERPROFILE unchanged", () => {
  assert.equal(probe(require("node:path").join(__dirname, ".."), "./bin/locus.js"), home);
  assert.equal(
    probe(require("node:path").join(__dirname, "..", "..", "npm-mcp"), "./bin/locus-mcp.js"),
    home,
  );
});
