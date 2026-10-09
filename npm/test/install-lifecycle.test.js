// Offline installer tests. All homes, caches, PATHs and child processes are synthetic.
const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");
const crypto = require("node:crypto");
const { EventEmitter } = require("node:events");
const fs = require("node:fs");
const { tmpdir } = require("node:os");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");
const root = path.join(__dirname, "..", "..");
const wrappers = [
  ["locus", path.join(root, "npm/bin/locus.js")],
  ["locus-mcp", path.join(root, "npm-mcp/bin/locus-mcp.js")],
];
const hash = (data) => crypto.createHash("sha256").update(data).digest("hex");

function nativeBytes(platform = "linux") {
  const bytes = Buffer.alloc(96);
  if (platform === "win32") {
    bytes.write("MZ"); bytes.writeUInt32LE(64, 0x3c); bytes.write("PE\0\0", 64);
  } else if (platform === "darwin") {
    bytes.writeUInt32BE(0xcffaedfe, 0);
  } else {
    bytes.set([0x7f, 0x45, 0x4c, 0x46, 2, 1, 1]);
  }
  return bytes;
}

function fixture(t, name, file, options = {}) {
  const home = fs.mkdtempSync(path.join(tmpdir(), "locus install space "));
  t.after(() => fs.rmSync(home, { recursive: true, force: true }));
  const first = path.join(home, "npm bin");
  const second = path.join(home, "native bin");
  fs.mkdirSync(first); fs.mkdirSync(second);
  const calls = [];
  const logs = [];
  const source = fs.readFileSync(file, "utf8");
  const processStub = {
    platform: options.platform || "linux", arch: options.arch || "x64",
    env: { HOME: home, USERPROFILE: home, LOCUS_HOME: path.join(home, "state"),
      CARGO_HOME: path.join(home, ".cargo"), PATH: [first, second].join(path.delimiter) },
  };
  const child = {
    execFileSync(binary, args, opts) {
      calls.push([binary, args]);
      if (options.extract && binary === "tar") {
        if (options.extractFailure) throw new Error("synthetic extraction failure");
        // GNU tar starts gzip via PATH; BSD tar's built-in gzip masked this
        // dependency locally. Use only system tools, retaining synthetic env.
        if (options.extract === "real") return execFileSync("/usr/bin/tar", args, {
          ...opts, env: { ...processStub.env, PATH: "/usr/bin:/bin" },
        });
        const dir = args[args.indexOf("-C") + 1];
        fs.writeFileSync(path.join(dir, name), options.binary);
        return Buffer.alloc(0);
      }
      // Native format fixtures are never executed. Stub only the exact CLI probe.
      assert.ok(binary.startsWith(home), `unexpected child process ${binary}`);
      assert.deepEqual(Array.from(args), ["--version"]);
      if (options.versionProbeFailure) throw new Error("synthetic version failure");
      return Buffer.from(options.version || "locus 0.5.0\n");
    },
    spawnSync() { throw new Error("No automatic Cargo/which child process allowed"); },
  };
  const https = {
    get(url, callback) {
      calls.push(["https", url]);
      if (!options.archive) throw new Error("offline fixture: network unavailable");
      const request = new EventEmitter();
      request.setTimeout = () => request;
      request.destroy = (err) => request.emit("error", err);
      queueMicrotask(() => {
        const response = new EventEmitter();
        response.statusCode = 200;
        response.headers = {};
        response.resume = () => {};
        callback(response);
        response.emit("data", options.archive);
        response.emit("end");
      });
      return request;
    },
  };
  const module = { exports: {} };
  // The verified-download fixture substitutes only a test archive's digest;
  // production pins are independently checked against the reviewed formula.
  const testSource = options.extract
    ? source.replace(/1ce1994a49ad8edd8c30b0e56173c7e45edf2e123002dddc279390a720e361f0/g, hash(options.archive))
    : source;
  vm.runInNewContext(testSource, {
    require(id) { return id === "child_process" ? child : id === "https" ? https : require(id); },
    module, exports: module.exports, __filename: file, process: processStub,
    Buffer, URL, console: { error: (msg) => logs.push(msg) },
  }, { filename: file });
  return { home, first, second, calls, logs, mod: module.exports, processStub,
    cache: path.join(home, ".locus/bin", name) };
}

for (const [name, file] of wrappers) {
  test(`${name}: skips own symlink and copied Node launcher, finds native PATH entry with spaces`, (t) => {
    const f = fixture(t, name, file);
    fs.symlinkSync(file, path.join(f.first, name));
    const native = path.join(f.second, name);
    fs.writeFileSync(native, nativeBytes(), { mode: 0o755 });
    assert.equal(f.mod.tryExistingOnPath(), native);
    fs.unlinkSync(path.join(f.first, name));
    fs.copyFileSync(file, path.join(f.first, name));
    fs.chmodSync(path.join(f.first, name), 0o755);
    assert.equal(f.mod.tryExistingOnPath(), native);
    if (name === "locus-mcp") assert.deepEqual(f.calls, [], "MCP candidates must never be probed with --version");
  });

  test(`${name}: Windows selects native exe rather than npm cmd launcher`, (t) => {
    const f = fixture(t, name, file, { platform: "win32", versionProbeFailure: true });
    fs.writeFileSync(path.join(f.first, `${name}.cmd`), '@node "%~dp0\\node_modules\\bin.js"');
    const native = path.join(f.second, `${name}.exe`);
    fs.writeFileSync(native, nativeBytes("win32"));
    if (name === "locus-mcp") assert.equal(f.mod.tryExistingOnPath(), native);
    else assert.equal(f.mod.tryExistingOnPath(), null, "a fake exe cannot report the exact CLI version");
  });

  test(`${name}: refuses shell trampolines, padded scripts and non-native bytes without executing`, (t) => {
    const f = fixture(t, name, file);
    const candidate = path.join(f.first, name);
    for (const bytes of ["#!/bin/sh\nexec env locus-mcp \"$@\"\n",
      "#!/bin/sh\n" + "# padding\n".repeat(1000) + "exec node shim.js\n", "MZ truncated", "arbitrary executable text"]) {
      fs.writeFileSync(candidate, bytes, { mode: 0o755 });
      assert.equal(f.mod.tryExistingOnPath(), null);
      assert.deepEqual(f.calls, []);
    }
  });

  test(`${name}: ignores relative PATH entries and recognizes Mach-O without executing MCP`, (t) => {
    const f = fixture(t, name, file, { platform: "darwin" });
    const candidate = path.join(f.second, name);
    fs.writeFileSync(candidate, nativeBytes("darwin"), { mode: 0o755 });
    f.processStub.env.PATH = "." + path.delimiter + path.relative(process.cwd(), f.second);
    assert.equal(f.mod.tryExistingOnPath(), null);
    assert.deepEqual(f.calls, []);
    f.processStub.env.PATH = f.second;
    assert.equal(f.mod.tryExistingOnPath(), candidate);
    if (name === "locus-mcp") assert.deepEqual(f.calls, []);
  });

  test(`${name}: refuses truncated PE and invalid signature offsets without a probe`, (t) => {
    const f = fixture(t, name, file, { platform: "win32" });
    const candidate = path.join(f.second, `${name}.exe`);
    for (const offset of [0, 32, 96, 1024 * 1024 + 1]) {
      const bytes = nativeBytes("win32");
      bytes.writeUInt32LE(offset, 0x3c);
      fs.writeFileSync(candidate, bytes);
      assert.equal(f.mod.tryExistingOnPath(), null);
      assert.deepEqual(f.calls, []);
    }
    fs.writeFileSync(candidate, "MZ truncated");
    assert.equal(f.mod.tryExistingOnPath(), null);
    assert.deepEqual(f.calls, []);
  });

  test(`${name}: checksum failure preserves old cache and never extracts or installs Cargo`, async (t) => {
    const f = fixture(t, name, file, { archive: Buffer.from("untrusted archive") });
    fs.mkdirSync(path.dirname(f.cache), { recursive: true });
    fs.writeFileSync(f.cache, "old cached binary");
    fs.writeFileSync(`${f.cache}.version`, "0.4.0\n");
    await assert.rejects(f.mod.ensureBinary(), /SHA-256 mismatch.*installation refused/);
    assert.equal(fs.readFileSync(f.cache, "utf8"), "old cached binary");
    assert.equal(f.calls.length, 1, JSON.stringify(f.calls));
    assert.equal(f.calls[0][0], "https");
    assert.doesNotMatch(f.calls[0][1], /sha256$/, "remote checksum cannot override package-owned pin");
    assert.deepEqual(fs.readdirSync(path.dirname(f.cache)).sort(), [name, `${name}.version`].sort());
  });

  test(`${name}: clean offline first run fails with a pinned manual command, no cache or Cargo side effects`, async (t) => {
    const f = fixture(t, name, file);
    fs.symlinkSync(file, path.join(f.first, name));
    await assert.rejects(f.mod.ensureBinary(), /offline fixture.*\nManual source install: cargo install .* --tag v0\.5\.0/);
    assert.equal(fs.existsSync(path.join(f.home, ".locus")), false);
    assert.equal(f.calls.length, 1);
  });

  test(`${name}: missing release target refuses without network or Cargo`, async (t) => {
    const f = fixture(t, name, file, { arch: "arm64" });
    await assert.rejects(f.mod.ensureBinary(), /No prebuilt.*No automatic source install/);
    assert.deepEqual(f.calls, []);
  });

  test(`${name}: unsupported OS refuses without guessing a release asset`, async (t) => {
    const f = fixture(t, name, file, { platform: "freebsd" });
    await assert.rejects(f.mod.ensureBinary(), /Unsupported platform: freebsd-x64/);
    assert.deepEqual(f.calls, []);
  });

  test(`${name}: failed extraction preserves stale cache and removes owned staging files`, async (t) => {
    const f = fixture(t, name, file, { archive: Buffer.from("verified fixture"), extract: true, extractFailure: true });
    fs.mkdirSync(path.dirname(f.cache), { recursive: true });
    fs.writeFileSync(f.cache, "old binary");
    fs.writeFileSync(`${f.cache}.version`, "0.4.0\n");
    await assert.rejects(f.mod.ensureBinary(), /synthetic extraction failure/);
    assert.equal(fs.readFileSync(f.cache, "utf8"), "old binary");
    assert.equal(fs.readFileSync(`${f.cache}.version`, "utf8"), "0.4.0\n");
    assert.deepEqual(fs.readdirSync(path.dirname(f.cache)).sort(), [name, `${name}.version`].sort());
    assert.equal(f.calls.filter(([binary]) => binary === "tar").length, 1);
  });

  test(`${name}: extracts a real nested tar archive in paths with spaces and cleans staging`, async (t) => {
    const staging = fs.mkdtempSync(path.join(tmpdir(), "locus archive space "));
    t.after(() => fs.rmSync(staging, { recursive: true, force: true }));
    const nested = "locus-x86_64-unknown-linux-gnu";
    fs.mkdirSync(path.join(staging, nested));
    const bytes = "#!/bin/sh\nprintf 'synthetic harmless fixture\\n'\n";
    fs.writeFileSync(path.join(staging, nested, name), bytes);
    const archive = path.join(staging, "fixture.tar.gz");
    execFileSync("/usr/bin/tar", ["czf", archive, "-C", staging, nested], {
      env: { HOME: staging, USERPROFILE: staging, LOCUS_HOME: path.join(staging, "state"), PATH: "/usr/bin:/bin" },
    });
    const f = fixture(t, name, file, { archive: fs.readFileSync(archive), extract: "real" });
    assert.equal(await f.mod.ensureBinary(), f.cache);
    assert.equal(fs.readFileSync(f.cache, "utf8"), bytes);
    assert.equal(fs.statSync(f.cache).mode & 0o777, 0o755);
    assert.deepEqual(fs.readdirSync(path.dirname(f.cache)).sort(), [name, `${name}.version`, `${name}.integrity.json`].sort());
  });

  test(`${name}: verified synthetic install caches integrity; reuse is offline and tampering fails closed`, async (t) => {
    const binary = Buffer.from("#!/bin/sh\nprintf 'locus 0.5.0\\n'\n");
    const f = fixture(t, name, file, { archive: Buffer.from("verified test archive"), extract: true, binary });
    assert.equal(await f.mod.ensureBinary(), f.cache);
    const firstCalls = f.calls.length;
    assert.equal(f.mod.isCachedBinaryStale(f.cache), false);
    assert.equal(await f.mod.ensureBinary(), f.cache);
    assert.equal(f.calls.length, firstCalls, "cache reuse must neither contact network nor probe MCP");
    const receipt = JSON.parse(fs.readFileSync(`${f.cache}.integrity.json`, "utf8"));
    assert.equal(receipt.binarySha256, hash(binary));
    assert.equal(receipt.archiveSha256, hash(Buffer.from("verified test archive")));
    assert.equal(receipt.version, "0.5.0");
    assert.deepEqual(fs.readdirSync(path.dirname(f.cache)).sort(), [name, `${name}.version`, `${name}.integrity.json`].sort());
    fs.appendFileSync(f.cache, "changed");
    assert.equal(f.mod.isCachedBinaryStale(f.cache), true);
    fs.writeFileSync(f.cache, binary);
    fs.writeFileSync(`${f.cache}.version`, "0.5.0\n");
    fs.unlinkSync(`${f.cache}.integrity.json`);
    assert.equal(f.mod.isCachedBinaryStale(f.cache), true, "legacy version-only cache is unverified");
    fs.writeFileSync(`${f.cache}.integrity.json`, JSON.stringify({ ...receipt, version: "0.4.0" }));
    assert.equal(f.mod.isCachedBinaryStale(f.cache), true);
    fs.writeFileSync(`${f.cache}.integrity.json`, JSON.stringify({ ...receipt, target: "aarch64-apple-darwin" }));
    assert.equal(f.mod.isCachedBinaryStale(f.cache), true);
  });
}

test("both package digest maps match the official formula's three v0.5.0 asset pins", () => {
  const formula = fs.readFileSync(path.join(root, "integrations/homebrew/Formula/locus.rb"), "utf8");
  for (const [, file] of wrappers) {
    const { RELEASE_SHA256 } = require(file);
    assert.equal(Object.keys(RELEASE_SHA256).length, 3);
    for (const [target, digest] of Object.entries(RELEASE_SHA256)) {
      assert.match(formula, new RegExp(`${target}:\\s+${digest}`));
    }
  }
  assert.deepEqual(require(wrappers[0][1]).RELEASE_SHA256, require(wrappers[1][1]).RELEASE_SHA256);
});

test("CLI PATH version comparison rejects 0.5.01 rather than accepting a substring", (t) => {
  const [name, file] = wrappers[0];
  const f = fixture(t, name, file, { version: "locus 0.5.01\n" });
  const native = path.join(f.first, name);
  fs.writeFileSync(native, nativeBytes(), { mode: 0o755 });
  assert.equal(f.mod.tryExistingOnPath(), null);
});
