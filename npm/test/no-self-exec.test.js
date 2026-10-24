// Regression test for the locus-cli 0.5.0 `locus --version` hang: the npm
// wrapper found itself on PATH (npm -g links bin/locus to it), executed that
// path, and recursed forever. Run the wrapper exactly as npm -g installs it,
// as a `locus` symlink on PATH, and require it to finish instead of spawning
// itself. Lives in its own file so it needs no change to the wrapper.
const { test } = require("node:test");
const assert = require("node:assert/strict");
const { spawnSync } = require("node:child_process");
const { chmodSync, copyFileSync, mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } = require("node:fs");
const { tmpdir } = require("node:os");
const { delimiter, join, resolve } = require("node:path");

const wrapper = process.env.LOCUS_WRAPPER_UNDER_TEST || resolve(__dirname, "..", "bin", "locus.js");

test("locus: a `locus` on PATH that is this wrapper is never executed (no self-exec loop)", { skip: process.platform === "win32" && "npm -g uses .cmd shims on Windows" }, () => {
  const home = mkdtempSync(join(tmpdir(), "locus-noloop-"));
  try {
    const bin = join(home, "prefix", "bin");
    mkdirSync(bin, { recursive: true });
    // npm -g copies the package and marks the bin executable; mirror that, since
    // `which` skips a non-executable file and would hide the loop.
    const lib = join(home, "prefix", "lib", "locus.js");
    mkdirSync(join(home, "prefix", "lib"));
    copyFileSync(wrapper, lib);
    chmodSync(lib, 0o755);
    symlinkSync(lib, join(bin, "locus"));
    // The wrapper's `#!/usr/bin/env node` must find this same node.
    const nodeDir = join(home, "node-bin");
    mkdirSync(nodeDir);
    symlinkSync(process.execPath, join(nodeDir, "node"));
    // A cargo that fails at once, so the fallback never builds anything for real.
    writeFileSync(join(nodeDir, "cargo"), "#!/bin/sh\nexit 1\n", { mode: 0o755 });
    // Pretend to be linux-arm64 (no prebuilt asset) so nothing is downloaded;
    // with no cargo on PATH the wrapper must fail fast with the fallback message.
    const preload = join(home, "arm64.js");
    writeFileSync(preload, 'Object.defineProperty(process, "platform", { value: "linux" }); Object.defineProperty(process, "arch", { value: "arm64" });\n');
    // A marker each wrapper process appends to, so recursion is counted, not guessed.
    const marker = join(home, "starts.log");
    const counter = join(home, "count.js");
    writeFileSync(counter, `require("fs").appendFileSync(${JSON.stringify(marker)}, "x");\n`);
    const result = spawnSync(process.execPath, ["--require", preload, "--require", counter, join(bin, "locus"), "--version"], {
      env: {
        HOME: home,
        USERPROFILE: home,
        PATH: [bin, nodeDir, "/usr/bin", "/bin"].join(delimiter),
        NODE_OPTIONS: `--require ${counter}`,
        LOCUS_NPM_NO_LINK: "1",
      },
      encoding: "utf8",
      timeout: 20_000,
      killSignal: "SIGKILL",
    });
    let starts = 0;
    try {
      starts = require("node:fs").readFileSync(marker, "utf8").length;
    } catch {}
    assert.notEqual(result.signal, "SIGKILL", `wrapper did not finish (self-exec loop?); ${starts} wrapper starts`);
    assert.equal(starts, 1, `the wrapper started ${starts} times; it executed itself from PATH`);
    assert.match(result.stderr, /No prebuilt locus binary for linux-arm64/);
  } finally {
    rmSync(home, { recursive: true, force: true });
  }
});
