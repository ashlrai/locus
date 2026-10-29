// Regression for #177. All installs run in disposable homes and package copies;
// preloaded transport/process doubles prevent network access or real cargo installs.
const assert = require("node:assert/strict");
const { spawnSync } = require("node:child_process");
const { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } = require("node:fs");
const { tmpdir } = require("node:os");
const { join } = require("node:path");
const test = require("node:test");
const { compareVersions } = require("../bin/locus.js");
const version = require("../package.json").version;

test("an older wrapper preserves a newer executable even with an old sidecar (#177)", (t) => {
  const home = mkdtempSync(join(tmpdir(), "locus-real-downgrade-"));
  t.after(() => rmSync(home, { recursive: true, force: true }));
  const bin = join(home, ".locus", "bin");
  const wrapperDir = join(home, "package", "bin");
  mkdirSync(bin, { recursive: true });
  mkdirSync(wrapperDir, { recursive: true });
  const binary = join(bin, process.platform === "win32" ? "locus.exe" : "locus");
  const wrapper = join(wrapperDir, "locus.js");
  // Node is a real executable whose --version output is accepted by the wrapper.
  // The package requires Node >=16, so its version is newer than this wrapper.
  assert.equal(compareVersions(process.version, version), 1);
  copyFileSync(process.execPath, binary);
  chmodSync(binary, 0o755);
  copyFileSync(join(__dirname, "..", "bin", "locus.js"), wrapper);
  writeFileSync(`${binary}.version`, "0.1.0\n");
  const original = readFileSync(binary);
  const attempts = join(home, "attempts");
  const preload = join(home, "block-external.js");
  writeFileSync(preload, `
    const fs = require("node:fs");
    const block = () => { fs.writeFileSync(${JSON.stringify(attempts)}, "attempted"); throw new Error("unexpected external action"); };
    require("node:https").get = block;
    require("node:child_process").spawnSync = block;
  `);
  const result = spawnSync(process.execPath, ["--require", preload, wrapper, "--help"], {
    env: { HOME: home, USERPROFILE: home, PATH: "", LOCUS_NPM_NO_LINK: "1" },
    encoding: "utf8",
    timeout: 15_000,
  });
  assert.equal(result.status, 1, result.stderr);
  assert.match(result.stderr, /Refusing to replace.*newer/);
  assert.ok(result.stderr.includes(process.version.slice(1)));
  assert.deepEqual(readFileSync(binary), original);
  assert.equal(readFileSync(`${binary}.version`, "utf8"), "0.1.0\n");
  assert.equal(existsSync(attempts), false, "must not search PATH, download, or run cargo");
});

test("version comparison follows SemVer rather than string or substring order", () => {
  for (const [left, right, expected] of [
    ["0.5.10", "0.5.9", 1],
    ["0.10.0", "0.9.0", 1],
    ["1.0.0", "0.99.99", 1],
    ["10.5.1", "0.5.1", 1],
    ["0.5.1-rc.2", "0.5.1-rc.10", -1],
    ["0.5.1-rc.1", "0.5.1", -1],
    ["0.5.1", "0.5.1-rc.1", 1],
    ["0.5.1-1", "0.5.1-alpha", -1],
    ["0.5.1-alpha", "0.5.1-beta", -1],
    ["0.5.1-alpha", "0.5.1-alpha.1", -1],
    ["0.5.1+one", "0.5.1+two", 0],
    ["locus 0.5.1", "v0.5.1", 0],
    ["9007199254740993.0.0", "9007199254740992.0.0", 1],
    ["garbage 0.5.1", "0.5.1", null],
    ["0.5", "0.5.1", null],
    ["0.05.1", "0.5.1", null],
    ["0.5.1-01", "0.5.1", null],
  ]) assert.equal(compareVersions(left, right), expected, `${left} vs ${right}`);
});

function fixture(t, { installed = "99.0.0", sidecar, companion, pathVersion, changedDuringDownload } = {}) {
  const home = mkdtempSync(join(tmpdir(), "locus-downgrade-"));
  t.after(() => rmSync(home, { recursive: true, force: true }));
  const pkg = join(home, "package");
  mkdirSync(join(pkg, "bin"), { recursive: true });
  for (const file of ["install.js", "bin/locus.js"]) {
    copyFileSync(join(__dirname, "..", file), join(pkg, file));
  }
  const cache = join(home, ".locus", "bin");
  mkdirSync(cache, { recursive: true });
  const binary = join(cache, "locus");
  if (installed !== false) writeFileSync(binary, "original binary");
  if (sidecar !== undefined) writeFileSync(`${binary}.version`, `${sidecar}\n`);
  if (companion) {
    writeFileSync(join(cache, "locus-mcp"), "original companion");
    writeFileSync(join(cache, "locus-mcp.version"), `${companion}\n`);
  }
  const pathBinary = pathVersion === undefined ? null : join(home, "pathbin", "locus");
  if (pathBinary) {
    mkdirSync(join(home, "pathbin"), { recursive: true });
    writeFileSync(pathBinary, "path binary");
  }
  const trace = join(home, "trace.log");
  const preload = join(home, "preload.js");
  writeFileSync(preload, `
    const fs = require("node:fs");
    const cp = require("node:child_process");
    const { join } = require("node:path");
    const { EventEmitter } = require("node:events");
    Object.defineProperty(process, "platform", { value: "linux" });
    Object.defineProperty(process, "arch", { value: "x64" });
    const binary = ${JSON.stringify(binary)};
    const pathBinary = ${JSON.stringify(pathBinary)};
    const record = (event) => fs.appendFileSync(${JSON.stringify(trace)}, event + "\\n");
    cp.execFileSync = (file, args) => {
      if (file === binary || file === pathBinary || file === join(${JSON.stringify(cache)}, "locus-mcp")) {
        if (args.length === 1 && args[0] === "--version") {
          const installed = file === binary ? ${JSON.stringify(installed)} : file === pathBinary ? ${JSON.stringify(pathVersion ?? null)} : ${JSON.stringify(companion)};
          if (!installed) throw new Error("version probe failed");
          return Buffer.from("locus " + installed + "\\n");
        }
        record((file === pathBinary ? "path native " : "native ") + JSON.stringify(args));
        return Buffer.alloc(0);
      }
      if (file === "tar") {
        const destination = args[args.indexOf("-C") + 1];
        fs.writeFileSync(join(destination, "locus"), "downloaded binary");
        fs.writeFileSync(join(destination, "locus-mcp"), "downloaded companion");
        return Buffer.alloc(0);
      }
      throw new Error("unexpected exec: " + file);
    };
    cp.spawnSync = (file) => {
      if (file === "which") return pathBinary ? { status: 0, stdout: pathBinary + "\\n" } : { status: 1, stdout: "" };
      record("unexpected spawn " + file);
      return { status: 1 };
    };
    require("node:https").get = (url, callback) => {
      record("download");
      const request = new EventEmitter();
      queueMicrotask(() => {
        const response = new EventEmitter();
        response.statusCode = url.endsWith(".sha256") ? 404 : 200;
        response.resume = () => {};
        callback(response);
        if (response.statusCode === 200) {
          if (${JSON.stringify(changedDuringDownload)} && fs.existsSync(binary)) {
            fs.writeFileSync(binary + ".version", "99.0.0\\n");
          }
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
      let script = join(pkg, entry === "install" ? "install.js" : "bin/locus.js");
      if (entry === "download") {
        script = join(home, "direct.js");
        writeFileSync(script, `require(${JSON.stringify(join(pkg, "bin/locus.js"))}).downloadReleaseBinary(${JSON.stringify(binary)}).catch((err) => { console.error(err.message); process.exitCode = 1; });`);
      }
      return spawnSync(process.execPath, ["--require", preload, script, ...args], {
        env: { HOME: home, USERPROFILE: home, PATH: "", ...(entry === "install" ? {} : { LOCUS_NPM_NO_LINK: "1" }) },
        encoding: "utf8",
        timeout: 15_000,
      });
    },
    trace() { return existsSync(trace) ? readFileSync(trace, "utf8") : ""; },
  };
}

for (const entry of ["wrapper", "install", "download"]) {
  test(`${entry}: refuses a newer binary without changing it or its sidecar`, (t) => {
    const f = fixture(t, { sidecar: "99.0.0" });
    const result = f.run(entry, ["--help"]);
    assert.equal(result.status, entry === "install" ? 0 : 1, result.stderr);
    assert.match(result.stderr + result.stdout, /Refusing to replace.*99\.0\.0.*newer/);
    assert.ok((result.stderr + result.stdout).includes(`npm's v${version}`));
    assert.match(result.stderr + result.stdout, /--force/);
    assert.equal(readFileSync(f.binary, "utf8"), "original binary");
    assert.equal(readFileSync(`${f.binary}.version`, "utf8"), "99.0.0\n");
    assert.equal(f.trace(), "", "must not download, execute a command, or run cargo");
  });
}

test("a cache one patch newer survives both wrapper and postinstall", (t) => {
  const [major, minor, patch] = version.split("-")[0].split(".").map(Number);
  const newer = `${major}.${minor}.${patch + 1}`;
  for (const entry of ["wrapper", "install"]) {
    const f = fixture(t, { installed: newer, sidecar: newer });
    const result = f.run(entry, ["--version"]);
    assert.equal(result.status, entry === "install" ? 0 : 1, result.stderr);
    assert.ok((result.stderr + result.stdout).includes(`installed v${newer} is newer`));
    assert.equal(readFileSync(f.binary, "utf8"), "original binary");
    assert.equal(readFileSync(`${f.binary}.version`, "utf8"), `${newer}\n`);
    assert.equal(f.trace(), "");
  }
});

for (const sidecar of [undefined, "0.1.0", "invalid"]) {
  test(`newer executable is protected with sidecar ${sidecar}`, (t) => {
    const f = fixture(t, { sidecar });
    assert.equal(f.run().status, 1);
    assert.equal(readFileSync(f.binary, "utf8"), "original binary");
    assert.equal(f.trace(), "");
  });
}

test("sidecar protects a newer version when the executable cannot report it", (t) => {
  const f = fixture(t, { installed: null, sidecar: "99.0.0" });
  assert.equal(f.run().status, 1);
  assert.equal(f.trace(), "");
});

test("unknown installed versions are preserved with an actionable refusal", (t) => {
  const f = fixture(t, { installed: null, sidecar: "garbage" });
  const result = f.run();
  assert.equal(result.status, 1);
  assert.match(result.stderr, /version could not be determined.*--force/);
  assert.equal(readFileSync(f.binary, "utf8"), "original binary");
  assert.equal(f.trace(), "");
});

for (const entry of ["wrapper", "install"]) {
  test(`${entry}: an explicit leading --force permits replacement`, (t) => {
    const f = fixture(t, { sidecar: "99.0.0", companion: "99.0.0" });
    const result = f.run(entry, ["--force", "status"]);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(readFileSync(f.binary, "utf8"), "downloaded binary");
    assert.equal(readFileSync(`${f.binary}.version`, "utf8"), `${version}\n`);
    assert.equal(readFileSync(join(f.binary, "..", "locus-mcp"), "utf8"), "downloaded companion");
    assert.match(f.trace(), /download/);
    assert.doesNotMatch(f.trace(), /unexpected spawn|native.*--force/);
    if (entry === "wrapper") assert.match(f.trace(), /native \["status"\]/);
  });
}

test("a native subcommand's --force never permits a downgrade", (t) => {
  const f = fixture(t);
  const result = f.run("wrapper", ["leave", "--force"]);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /Refusing to replace/);
  assert.equal(f.trace(), "");
});

test("a current binary is reused and native subcommand flags pass through", (t) => {
  const f = fixture(t, { installed: version });
  const result = f.run("wrapper", ["leave", "--force"]);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(f.trace(), 'native ["leave","--force"]\n');
});

for (const pathVersion of [version, "99.0.0"]) {
  test(`an older cache yields to a PATH binary at ${pathVersion}`, (t) => {
    const f = fixture(t, { installed: "0.1.0", pathVersion });
    const result = f.run("wrapper", ["status"]);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(readFileSync(f.binary, "utf8"), "original binary");
    assert.equal(f.trace(), 'path native ["status"]\n');
  });
}

for (const pathVersion of ["0.1.0", null]) {
  test(`an older or unversioned PATH binary (${pathVersion}) is not selected`, (t) => {
    const f = fixture(t, { installed: false, pathVersion });
    const result = f.run("wrapper", ["status"]);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(readFileSync(f.binary, "utf8"), "downloaded binary");
    assert.match(f.trace(), /download/);
    assert.doesNotMatch(f.trace(), /path native/);
  });
}

test("a protected cache prevents selection of an otherwise eligible PATH binary", (t) => {
  const f = fixture(t, { installed: "99.0.0", pathVersion: version });
  const result = f.run("wrapper", ["status"]);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /Refusing to replace/);
  assert.equal(f.trace(), "");
});

for (const installed of [false, "0.1.0", `${version}-rc.1`]) {
  test(`fresh install or upgrade from ${installed} still succeeds`, (t) => {
    const f = fixture(t, { installed });
    const result = f.run("wrapper", ["status"]);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(readFileSync(f.binary, "utf8"), "downloaded binary");
    assert.equal(readFileSync(`${f.binary}.version`, "utf8"), `${version}\n`);
  });
}

test("a newer companion binary is preserved during a CLI upgrade", (t) => {
  const f = fixture(t, { installed: "0.1.0", companion: "99.0.0" });
  const result = f.run("wrapper", ["status"]);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(readFileSync(f.binary, "utf8"), "downloaded binary");
  assert.equal(readFileSync(join(f.binary, "..", "locus-mcp"), "utf8"), "original companion");
  assert.match(result.stderr, /Refusing to replace.*locus-mcp/);
});

test("replacement is refused again if the cache becomes newer during download", (t) => {
  const f = fixture(t, { installed: null, sidecar: "0.1.0", changedDuringDownload: true });
  const result = f.run("wrapper", ["status"]);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /Refusing to replace.*99\.0\.0/);
  assert.equal(readFileSync(f.binary, "utf8"), "original binary");
  assert.doesNotMatch(f.trace(), /unexpected spawn/);
});
