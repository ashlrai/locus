// Integrity errors must not install or fall back to cargo for either wrapper.
const assert = require("node:assert/strict");
const { spawnSync } = require("node:child_process");
const { mkdtempSync, rmSync } = require("node:fs");
const { tmpdir } = require("node:os");
const { join } = require("node:path");
const test = require("node:test");

const root = join(__dirname, "..", "..");
for (const [name, file] of [
  ["locus", join(root, "npm", "bin", "locus.js")],
  ["locus-mcp", join(root, "npm-mcp", "bin", "locus-mcp.js")],
]) {
  for (const mode of ["network", "missing", "malformed", "wrong-filename", "mismatch", "valid"]) {
    for (const cached of [false, true]) {
      test(`${name}: ${cached ? "update" : "fresh install"}, ${mode} checksum ${mode === "valid" ? "installs" : "fails closed without installing or invoking cargo"}`, () => {
        const home = mkdtempSync(join(tmpdir(), "locus-integrity-"));
        try {
          const script = `
            const assert = require("node:assert/strict");
            const crypto = require("node:crypto");
            const { EventEmitter } = require("node:events");
            const fs = require("node:fs");
            const path = require("node:path");
            const cp = require("node:child_process");
            const https = require("node:https");
            Object.defineProperty(process, "platform", { value: "linux" });
            Object.defineProperty(process, "arch", { value: "x64" });
            const name = ${JSON.stringify(name)};
            const mode = ${JSON.stringify(mode)};
            const cached = ${JSON.stringify(cached)};
            const binary = path.join(process.env.HOME, ".locus", "bin", name);
            fs.mkdirSync(path.dirname(binary), { recursive: true });
            if (cached) {
              fs.writeFileSync(binary, "old binary");
              fs.writeFileSync(binary + ".version", "0.0.1\\n");
            }
            const archive = Buffer.from("release archive fixture");
            const requests = [];
            let cargoAttempted = false;
            cp.spawnSync = (command) => {
              if (command === "which") return { status: 1, stdout: "" };
              cargoAttempted = true;
              throw new Error("unexpected cargo invocation");
            };
            cp.execFileSync = (command, args) => {
              if (command === binary) return Buffer.from(name + " 0.0.1\\n");
              assert.equal(command, "tar");
              const directory = path.join(args[args.indexOf("-C") + 1], "locus-x86_64-unknown-linux-gnu");
              fs.mkdirSync(directory);
              fs.writeFileSync(path.join(directory, name), "verified replacement");
              return Buffer.alloc(0);
            };
            https.get = (url, callback) => {
              requests.push(url);
              const request = new EventEmitter();
              process.nextTick(() => {
                if (mode === "network" && url.endsWith(".sha256")) {
                  request.emit("error", new Error("fixture connection failure"));
                  return;
                }
                const response = new EventEmitter();
                response.statusCode = mode === "missing" && url.endsWith(".sha256") ? 404 : 200;
                response.headers = {};
                response.resume = () => {};
                callback(response);
                if (response.statusCode !== 200) return;
                let body = archive;
                if (url.endsWith(".sha256")) {
                  const digest = crypto.createHash("sha256").update(mode === "mismatch" ? "wrong" : archive).digest("hex");
                  const filename = mode === "wrong-filename" ? "other.tar.gz" : path.basename(url.slice(0, -7));
                  body = Buffer.from(mode === "malformed" ? "not a checksum\\n" : digest + "  " + filename + "\\n");
                }
                response.emit("data", body);
                response.emit("end");
              });
              return request;
            };
            const installer = require(${JSON.stringify(file)});
            (async () => {
              if (mode === "valid") {
                assert.equal(await installer.ensureBinary({ force: true }), binary);
                assert.equal(fs.readFileSync(binary, "utf8"), "verified replacement");
                assert.equal(fs.readFileSync(binary + ".version", "utf8").trim(), ${JSON.stringify(require(join(root, name === "locus" ? "npm" : "npm-mcp", "package.json")).version)});
              } else {
                await assert.rejects(installer.ensureBinary({ force: true }), (err) => {
                  assert.equal(err.code, "LOCUS_NPM_CHECKSUM");
                  assert.match(err.message, /SHA-256/);
                  assert.match(err.message, /retry/i);
                  return true;
                });
                if (cached) {
                  assert.equal(fs.readFileSync(binary, "utf8"), "old binary");
                  assert.equal(fs.readFileSync(binary + ".version", "utf8"), "0.0.1\\n");
                } else {
                  assert.equal(fs.existsSync(binary), false, "no executable installed");
                  assert.equal(fs.existsSync(binary + ".version"), false, "no version sidecar installed");
                }
              }
              assert.equal(cargoAttempted, false);
              assert.equal(requests.length, mode === "mismatch" || mode === "valid" ? 2 : 1);
              assert.match(requests[0], /\\.sha256$/);
              assert.equal(fs.existsSync(path.join(path.dirname(binary), "locus-x86_64-unknown-linux-gnu.tar.gz")), false);
            })().catch((err) => { console.error(err); process.exitCode = 1; });
          `;
          const result = spawnSync(process.execPath, ["-e", script], {
            env: { ...process.env, HOME: home, USERPROFILE: home, PATH: "" },
            encoding: "utf8",
            timeout: 10_000,
          });
          assert.equal(result.status, 0, result.stderr);
          assert.equal(result.signal, null);
        } finally {
          rmSync(home, { recursive: true, force: true });
        }
      });
    }
  }
}

for (const [name, packageDir, entry] of [
  ["locus", "npm", "bin/locus.js"],
  ["locus", "npm", "install.js"],
  ["locus-mcp", "npm-mcp", "bin/locus-mcp.js"],
]) {
  for (const cached of [false, true]) {
    test(`${name} ${entry}: checksum failure exits unsuccessfully on ${cached ? "update without activating the stale cache" : "fresh install"}`, () => {
      const fs = require("node:fs");
      const home = mkdtempSync(join(tmpdir(), "locus-integrity-entry-"));
      try {
        const pkg = join(home, "package");
        fs.mkdirSync(join(pkg, "bin"), { recursive: true });
        fs.copyFileSync(join(root, packageDir, "install.js"), join(pkg, "install.js"));
        fs.copyFileSync(join(root, packageDir, "bin", `${name}.js`), join(pkg, "bin", `${name}.js`));
        const binary = join(home, ".locus", "bin", name);
        fs.mkdirSync(join(home, ".locus", "bin"), { recursive: true });
        if (cached) {
          fs.writeFileSync(binary, "old binary");
          fs.writeFileSync(binary + ".version", "0.0.1\n");
        }
        const preload = join(home, "preload.js");
        fs.writeFileSync(preload, `
          const cp = require("node:child_process");
          const { EventEmitter } = require("node:events");
          Object.defineProperty(process, "platform", { value: "linux" });
          Object.defineProperty(process, "arch", { value: "x64" });
          cp.execFileSync = (file, args) => {
            if (file === ${JSON.stringify(binary)} && args.length === 1 && args[0] === "--version") return Buffer.from("${name} 0.0.1\\n");
            throw new Error("must not activate stale binary or extract archive");
          };
          cp.spawnSync = (file) => {
            if (file === "which") return { status: 1, stdout: "" };
            throw new Error("must not invoke cargo");
          };
          require("node:https").get = (url, callback) => {
            if (!url.endsWith(".sha256")) throw new Error("must not fetch archive");
            const request = new EventEmitter();
            process.nextTick(() => {
              const response = new EventEmitter();
              response.statusCode = 404;
              response.headers = {};
              response.resume = () => {};
              callback(response);
            });
            return request;
          };
        `);
        const result = spawnSync(process.execPath, ["--require", preload, join(pkg, entry), "status"], {
          env: { HOME: home, USERPROFILE: home, PATH: "" },
          encoding: "utf8",
          timeout: 10_000,
        });
        assert.equal(result.status, 1, result.stderr);
        assert.match(result.stderr, /SHA-256.*retry/i);
        assert.doesNotMatch(result.stderr, /cargo|must not/);
        if (cached) {
          assert.equal(fs.readFileSync(binary, "utf8"), "old binary");
          assert.equal(fs.readFileSync(binary + ".version", "utf8"), "0.0.1\n");
        } else {
          assert.equal(fs.existsSync(binary), false, "no executable installed");
          assert.equal(fs.existsSync(binary + ".version"), false, "no version sidecar installed");
        }
        assert.equal(fs.lstatSync(join(pkg, "bin", `${name}.js`)).isSymbolicLink(), false);
      } finally {
        rmSync(home, { recursive: true, force: true });
      }
    });
  }
}
