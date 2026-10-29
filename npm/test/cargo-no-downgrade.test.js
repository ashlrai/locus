// Cargo fallback must not replace an independently installed binary or a cache
// that becomes newer while the source build is running.
const assert = require("node:assert/strict");
const { spawnSync } = require("node:child_process");
const { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } = require("node:fs");
const { tmpdir } = require("node:os");
const { join } = require("node:path");
const test = require("node:test");

for (const [name, wrapper] of [
  ["locus", "../../npm/bin/locus.js"],
  ["locus-mcp", "../../npm-mcp/bin/locus-mcp.js"],
]) {
  const version = require(name === "locus" ? "../package.json" : "../../npm-mcp/package.json").version;
  for (const scenario of ["upgrade", "download-fails", "newer-build", "older-build", "forced-older-build", "unknown-build", "fresh-unknown", "concurrent-update", "cargo-fails"]) {
    test(`${name}: isolated cargo fallback ${scenario}`, (t) => {
      const home = mkdtempSync(join(tmpdir(), "locus-cargo-test-"));
      t.after(() => rmSync(home, { recursive: true, force: true }));
      const cache = join(home, ".locus", "bin", name);
      const cargoRoot = join(home, "custom-cargo");
      const cargoBinary = join(cargoRoot, "bin", name);
      const trace = join(home, "trace");
      mkdirSync(join(home, ".locus", "bin"), { recursive: true });
      mkdirSync(join(cargoRoot, "bin"), { recursive: true });
      if (scenario !== "fresh-unknown") writeFileSync(cache, "0.1.0");
      writeFileSync(`${cache}.version`, "0.1.0\n");
      writeFileSync(cargoBinary, "99.0.0");
      const preload = join(home, "preload.js");
      writeFileSync(preload, `
        const fs = require("node:fs");
        const cp = require("node:child_process");
        const path = require("node:path");
        Object.defineProperty(process, "platform", { value: "linux" });
        Object.defineProperty(process, "arch", { value: ${JSON.stringify(scenario === "download-fails" ? "x64" : "arm64")} });
        require("node:https").get = (url, callback) => {
          const { EventEmitter } = require("node:events");
          const request = new EventEmitter();
          queueMicrotask(() => {
            const response = new EventEmitter();
            response.statusCode = 404;
            response.resume = () => {};
            callback(response);
          });
          return request;
        };
        cp.execFileSync = (file, args) => {
          if (args.length === 1 && args[0] === "--version") {
            const version = fs.readFileSync(file, "utf8");
            if (version === "unknown") throw new Error("no version");
            return Buffer.from(${JSON.stringify(name)} + " " + version + "\\n");
          }
          fs.writeFileSync(${JSON.stringify(trace)}, file);
          return Buffer.alloc(0);
        };
        cp.spawnSync = (file, args) => {
          if (file === "which") return { status: 1, stdout: "" };
          if (file !== "cargo") throw new Error("unexpected process");
          if (args.includes("--force") || !args.includes("--root")) throw new Error("unsafe cargo invocation");
          const root = args[args.indexOf("--root") + 1];
          if (root === ${JSON.stringify(cargoRoot)}) throw new Error("shared cargo root");
          if (${JSON.stringify(scenario)} === "cargo-fails") return { status: 1 };
          fs.mkdirSync(path.join(root, "bin"), { recursive: true });
          const scenario = ${JSON.stringify(scenario)};
          const version = ["older-build", "forced-older-build"].includes(scenario) ? "0.0.1" :
            ["unknown-build", "fresh-unknown"].includes(scenario) ? "unknown" :
            scenario === "newer-build" ? "99.0.0" : ${JSON.stringify(version)};
          fs.writeFileSync(path.join(root, "bin", ${JSON.stringify(name)}), version);
          if (${JSON.stringify(scenario)} === "concurrent-update") fs.writeFileSync(${JSON.stringify(cache)}, "99.0.0");
          return { status: 0 };
        };
      `);
      const args = scenario === "forced-older-build" ? ["--force", "status"] : ["status"];
      const result = spawnSync(process.execPath, ["--require", preload, join(__dirname, wrapper), ...args], {
        env: { HOME: home, USERPROFILE: home, PATH: "", CARGO_INSTALL_ROOT: cargoRoot, LOCUS_NPM_NO_LINK: "1" },
        encoding: "utf8",
        timeout: 15_000,
      });
      const succeeds = ["upgrade", "download-fails", "newer-build", "forced-older-build", "fresh-unknown"].includes(scenario);
      assert.equal(result.status, succeeds ? 0 : 1, result.stderr);
      assert.equal(readFileSync(cargoBinary, "utf8"), "99.0.0");
      const expected = ["upgrade", "download-fails"].includes(scenario) ? version : scenario === "newer-build" || scenario === "concurrent-update" ? "99.0.0" : scenario === "forced-older-build" ? "0.0.1" : scenario === "fresh-unknown" ? "unknown" : "0.1.0";
      assert.equal(readFileSync(cache, "utf8"), expected);
      if (scenario === "fresh-unknown") assert.equal(existsSync(`${cache}.version`), false);
      else assert.equal(readFileSync(`${cache}.version`, "utf8"), `${succeeds ? expected : "0.1.0"}\n`);
      assert.equal(existsSync(trace), succeeds);
      assert.equal(readdirSync(join(home, ".locus", "bin")).some((file) => file.startsWith(".cargo-install-")), false);
      if (["older-build", "unknown-build", "concurrent-update"].includes(scenario)) assert.match(result.stderr, /Refusing to replace/);
    });
  }
}
