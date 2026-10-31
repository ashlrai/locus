// Regression for #177 on @ashlrai/locus-mcp: the MCP wrapper must never replace
// a newer ~/.locus/bin/locus-mcp. Lives under npm/test because CI runs
// `node --test npm/test` only; the fixture spawns ../../npm-mcp/bin/locus-mcp.js
// in disposable homes with preloaded transport/process doubles (no network, no cargo).
const assert = require("node:assert/strict");
const { spawnSync } = require("node:child_process");
const { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } = require("node:fs");
const { tmpdir } = require("node:os");
const { join } = require("node:path");
const test = require("node:test");
const { compareVersions } = require("../../npm-mcp/bin/locus-mcp.js");
const version = require("../../npm-mcp/package.json").version;

test("mcp wrapper compares versions with SemVer and its own name prefix", () => {
  for (const [left, right, expected] of [
    ["0.5.10", "0.5.9", 1],
    ["locus-mcp 0.6.0", "0.5.2", 1],
    ["locus-mcp 0.5.2", "v0.5.2", 0],
    ["0.5.1-rc.1", "0.5.1", -1],
    ["garbage 0.5.1", "0.5.1", null],
  ]) assert.equal(compareVersions(left, right), expected, `${left} vs ${right}`);
});

function fixture(t, { installed = "99.0.0", sidecar, pathVersion, changedDuringDownload } = {}) {
  const home = mkdtempSync(join(tmpdir(), "locus-mcp-downgrade-"));
  t.after(() => rmSync(home, { recursive: true, force: true }));
  const pkg = join(home, "package");
  mkdirSync(join(pkg, "bin"), { recursive: true });
  for (const file of ["install.js", "bin/locus-mcp.js"]) {
    copyFileSync(join(__dirname, "..", "..", "npm-mcp", file), join(pkg, file));
  }
  const cache = join(home, ".locus", "bin");
  mkdirSync(cache, { recursive: true });
  const binary = join(cache, "locus-mcp");
  if (installed !== false) writeFileSync(binary, "original mcp binary");
  if (sidecar !== undefined) writeFileSync(`${binary}.version`, `${sidecar}\n`);
  const pathBinary = pathVersion === undefined ? null : join(home, "pathbin", "locus-mcp");
  if (pathBinary) {
    mkdirSync(join(home, "pathbin"), { recursive: true });
    writeFileSync(pathBinary, "path mcp binary");
  }
  const trace = join(home, "trace.log");
  const preload = join(home, "preload.js");
  writeFileSync(preload, `
    const fs = require("node:fs");
    const cp = require("node:child_process");
    const { join } = require("node:path");
    const { EventEmitter } = require("node:events");
    const crypto = require("node:crypto");
    const path = require("node:path");
    Object.defineProperty(process, "platform", { value: "linux" });
    Object.defineProperty(process, "arch", { value: "x64" });
    const binary = ${JSON.stringify(binary)};
    const pathBinary = ${JSON.stringify(pathBinary)};
    let cacheVersion = ${JSON.stringify(installed)};
    const record = (event) => fs.appendFileSync(${JSON.stringify(trace)}, event + "\\n");
    cp.execFileSync = (file, args) => {
      if (args.length === 1 && args[0] === "--version") {
        const v = file === binary ? cacheVersion : file === pathBinary ? ${JSON.stringify(pathVersion ?? null)} : null;
        if (!v) throw new Error("version probe failed");
        return Buffer.from("locus-mcp " + v + "\\n");
      }
      if (file === "tar") {
        const destination = args[args.indexOf("-C") + 1];
        fs.writeFileSync(join(destination, "locus-mcp"), "downloaded mcp binary");
        return Buffer.alloc(0);
      }
      record("native " + file + " " + JSON.stringify(args));
      return Buffer.alloc(0);
    };
    cp.spawnSync = (file) => {
      if (file === "which" && pathBinary) return { status: 0, stdout: pathBinary + "\\n" };
      record("spawn " + file);
      return { status: 1, stdout: "" };
    };
    require("node:https").get = (url, callback) => {
      record("download");
      const request = new EventEmitter();
      queueMicrotask(() => {
        const response = new EventEmitter();
        response.statusCode = 200;
        response.headers = {};
        response.resume = () => {};
        callback(response);
        if (url.endsWith(".sha256")) {
          const hash = crypto.createHash("sha256").update("archive fixture").digest("hex");
          response.emit("data", Buffer.from(hash + "  " + path.basename(url.slice(0, -7)) + "\\n"));
          response.emit("end");
        } else {
          if (${JSON.stringify(changedDuringDownload)}) cacheVersion = "99.0.0";
          response.emit("data", Buffer.from("archive fixture"));
          response.emit("end");
        }
      });
      return request;
    };
  `);
  return {
    binary,
    run(entry = "wrapper", args = []) {
      const script = join(pkg, entry === "install" ? "install.js" : "bin/locus-mcp.js");
      return spawnSync(process.execPath, ["--require", preload, script, ...args], {
        env: { HOME: home, USERPROFILE: home, PATH: "" },
        encoding: "utf8",
        timeout: 15_000,
      });
    },
    trace() { return existsSync(trace) ? readFileSync(trace, "utf8") : ""; },
  };
}

test("wrapper keeps and runs a newer binary instead of replacing it", (t) => {
  const f = fixture(t, { installed: "99.0.0" });
  const result = f.run();
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stderr, /99\.0\.0.*newer than npm's v/);
  assert.equal(readFileSync(f.binary, "utf8"), "original mcp binary");
  assert.equal(existsSync(`${f.binary}.version`), false);
  assert.equal(f.trace(), `native ${f.binary} []\n`, "ran the kept binary; no download or cargo");
});

test("wrapper keeps a newer binary when only the sidecar reports it", (t) => {
  const f = fixture(t, { installed: null, sidecar: "99.0.0" });
  const result = f.run();
  assert.equal(result.status, 0, result.stderr);
  assert.equal(readFileSync(f.binary, "utf8"), "original mcp binary");
  assert.equal(readFileSync(`${f.binary}.version`, "utf8"), "99.0.0\n");
  assert.doesNotMatch(f.trace(), /download|spawn/);
});

for (const sidecar of [undefined, "garbage"]) {
  test(`an unversioned binary is kept and run (sidecar ${sidecar})`, (t) => {
    const f = fixture(t, { installed: null, sidecar });
    const result = f.run();
    assert.equal(result.status, 0, result.stderr);
    assert.equal(readFileSync(f.binary, "utf8"), "original mcp binary");
    assert.match(f.trace(), new RegExp(`native ${f.binary.replace(/\//g, "\\/")}`));
    assert.doesNotMatch(f.trace(), /download|spawn/);
  });
}

test("a current binary is reused without a download", (t) => {
  const f = fixture(t, { installed: version });
  const result = f.run();
  assert.equal(result.status, 0, result.stderr);
  assert.match(f.trace(), new RegExp(`native ${f.binary.replace(/\//g, "\\/")}`));
  assert.doesNotMatch(f.trace(), /download|spawn/);
});

for (const installed of ["0.1.0", `${version}-rc.1`]) {
  test(`a provably older binary (${installed}) is upgraded`, (t) => {
    const f = fixture(t, { installed });
    const result = f.run();
    assert.equal(result.status, 0, result.stderr);
    assert.equal(readFileSync(f.binary, "utf8"), "downloaded mcp binary");
    assert.equal(readFileSync(`${f.binary}.version`, "utf8"), `${version}\n`);
    assert.match(f.trace(), /download/);
  });
}

test("a leading --force replaces a newer binary, other positions do not", (t) => {
  const forced = fixture(t, { installed: "99.0.0" });
  const ok = forced.run("wrapper", ["--force"]);
  assert.equal(ok.status, 0, ok.stderr);
  assert.equal(readFileSync(forced.binary, "utf8"), "downloaded mcp binary");
  assert.equal(readFileSync(`${forced.binary}.version`, "utf8"), `${version}\n`);

  const notLeading = fixture(t, { installed: "99.0.0" });
  const kept = notLeading.run("wrapper", ["--supervised", "--force"]);
  assert.equal(kept.status, 0, kept.stderr);
  assert.equal(readFileSync(notLeading.binary, "utf8"), "original mcp binary");
  assert.doesNotMatch(notLeading.trace(), /download/);
  assert.match(notLeading.trace(), /native .*"--force"/, "non-leading flag reaches the server");
});

test("replacement is refused if the cache becomes newer during download", (t) => {
  const f = fixture(t, { installed: "0.1.0", sidecar: "0.1.0", changedDuringDownload: true });
  const result = f.run();
  assert.equal(result.status, 1);
  assert.match(result.stderr, /Refusing to replace.*99\.0\.0/);
  assert.equal(readFileSync(f.binary, "utf8"), "original mcp binary");
  assert.equal(readFileSync(`${f.binary}.version`, "utf8"), "0.1.0\n");
  assert.match(f.trace(), /download/);
  assert.doesNotMatch(f.trace(), /native|spawn cargo/);
});

test("a newer locus-mcp already on PATH is reused while the cache stays untouched", (t) => {
  const f = fixture(t, { installed: "0.1.0", sidecar: "0.1.0", pathVersion: "99.0.0" });
  const result = f.run();
  assert.equal(result.status, 0, result.stderr);
  assert.equal(readFileSync(f.binary, "utf8"), "original mcp binary");
  assert.match(f.trace(), /native .*pathbin.*\[\]/);
  assert.doesNotMatch(f.trace(), /download/);
});

test("postinstall keeps an existing newer binary", (t) => {
  const f = fixture(t, { installed: "99.0.0" });
  const result = f.run("install");
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /already installed/);
  assert.equal(readFileSync(f.binary, "utf8"), "original mcp binary");
  assert.equal(f.trace(), "");
});
