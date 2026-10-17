// Tests for the npm wrappers' platform handling (issue #50).
// Run: node --test npm/test
const assert = require("node:assert/strict");
const { spawnSync } = require("node:child_process");
const { mkdtempSync, readFileSync, rmSync, writeFileSync } = require("node:fs");
const { tmpdir } = require("node:os");
const { join } = require("node:path");
const test = require("node:test");

const root = join(__dirname, "..", "..");
const wrappers = {
  locus: join(root, "npm", "bin", "locus.js"),
  "locus-mcp": join(root, "npm-mcp", "bin", "locus-mcp.js"),
};

function releaseMatrixTargets() {
  const yml = readFileSync(join(root, ".github", "workflows", "release.yml"), "utf8");
  // Each build job lists its targets space-separated (both macOS targets
  // share one job), so collect every name from every `targets:` line.
  return [...yml.matchAll(/^\s*targets:\s*([\w -]+?)\s*$/gm)]
    .flatMap((m) => m[1].split(/\s+/))
    .sort();
}

for (const [name, file] of Object.entries(wrappers)) {
  const mod = require(file);

  test(`${name}: PREBUILT_TARGETS matches the release.yml matrix`, () => {
    assert.deepEqual([...mod.PREBUILT_TARGETS].sort(), releaseMatrixTargets());
  });

  test(`${name}: every prebuilt target is a supported target`, () => {
    const supported = Object.values(mod.SUPPORTED_TARGETS);
    for (const t of mod.PREBUILT_TARGETS) assert.ok(supported.includes(t), t);
    assert.equal(mod.hasPrebuiltBinary("aarch64-unknown-linux-gnu"), false);
    assert.equal(mod.hasPrebuiltBinary("x86_64-unknown-linux-gnu"), true);
  });

  test(`${name}: fallback message names the platform, version and cost`, () => {
    const msg = mod.cargoFallbackMessage({ platform: "linux", arch: "arm64" });
    assert.match(msg, new RegExp(`^No prebuilt ${name} binary for linux-arm64 in v\\d+\\.\\d+\\.\\d+; building from source with cargo`));
    assert.match(msg, /several minutes/);
    assert.match(msg, /rustup\.rs/);
    assert.match(mod.cargoFallbackMessage({ platform: "darwin", arch: "arm64" }, "download-failed"), /^Could not download the prebuilt/);
  });

  test(`${name}: on a platform without a prebuilt binary it explains the cargo fallback before trying it (no network)`, () => {
    const home = mkdtempSync(join(tmpdir(), "locus-wrapper-"));
    try {
      // Pretend to be linux-arm64, which has no release asset. PATH has no
      // locus and no cargo, so the fallback fails fast after the message.
      const preload = join(home, "arm64.js");
      writeFileSync(preload, 'Object.defineProperty(process, "platform", { value: "linux" }); Object.defineProperty(process, "arch", { value: "arm64" });\n');
      const result = spawnSync(process.execPath, ["--require", preload, file, "--help"], {
        env: { HOME: home, USERPROFILE: home, PATH: join(home, "empty-bin") },
        encoding: "utf8",
        timeout: 20_000,
      });
      assert.equal(result.status, 1, result.stderr);
      assert.match(result.stderr, new RegExp(`No prebuilt ${name} binary for linux-arm64`));
      assert.doesNotMatch(result.stderr, /Downloading/, "must not attempt a download that would 404");
      const messageAt = result.stderr.indexOf("No prebuilt");
      const cargoAt = result.stderr.indexOf("Falling back to: cargo install");
      assert.ok(messageAt >= 0 && cargoAt > messageAt, result.stderr);
    } finally {
      rmSync(home, { recursive: true, force: true });
    }
  });
}

test("source-install commands use a positional crate (cargo install has no --package flag)", () => {
  for (const file of Object.values(wrappers)) {
    const mod = require(file);
    assert.doesNotMatch(mod.INSTALL_FROM_SOURCE, /--package/);
    assert.match(mod.INSTALL_FROM_SOURCE, /^cargo install --git https:\/\/github\.com\/ashlrai\/locus locus(-cli|-mcp) --locked$/);
    assert.doesNotMatch(readFileSync(file, "utf8").split("function installViaCargo")[1].split("\n}\n")[0], /"--package"/);
  }
  for (const doc of ["README.md", "npm/README.md", "npm-mcp/README.md", "integrations/homebrew/README.md", "apps/web/public/index.html"]) {
    const text = readFileSync(join(root, doc), "utf8").replace(/\\\n\s*/g, " ");
    assert.doesNotMatch(text, /cargo install [^\n]*--package/, `${doc} still uses cargo install --package`);
  }
});

// Regression: quickstart then `enter` failed with a stale session authority
// because the Node wrapper, not the shell, was locus's parent process.
test("locus: linkNativeBin replaces the wrapper with a symlink to the binary", () => {
  if (process.platform === "win32") return;
  const { linkNativeBin } = require(wrappers.locus);
  const { lstatSync, readlinkSync } = require("node:fs");
  const dir = mkdtempSync(join(tmpdir(), "locus-link-"));
  try {
    const bin = join(dir, "locus-native");
    const target = join(dir, "locus.js");
    writeFileSync(bin, "#!/bin/sh\necho native\n", { mode: 0o755 });
    writeFileSync(target, "// wrapper\n");
    assert.equal(linkNativeBin(bin, target), true);
    assert.ok(lstatSync(target).isSymbolicLink());
    assert.equal(readlinkSync(target), bin);
    const out = spawnSync(target, [], { encoding: "utf8" });
    assert.equal(out.stdout.trim(), "native");
    assert.equal(linkNativeBin(bin, target), false, "idempotent");
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

// Regression: a root `npm install -g` linked the shared bin to
// /root/.locus/bin/locus (under a 0700 HOME), so every other user got
// "Permission denied". A world-reachable wrapper must not be linked to a
// binary other users cannot reach.
test("locus: linkNativeBin keeps a root-installed shared wrapper when the binary is in a private dir", () => {
  if (process.platform === "win32") return;
  const { linkNativeBin } = require(wrappers.locus);
  const { chmodSync, lstatSync, mkdirSync, realpathSync, statSync, unlinkSync } = require("node:fs");
  const { dirname } = require("node:path");
  const dir = mkdtempSync(join(tmpdir(), "locus-shared-"));
  try {
    chmodSync(dir, 0o755);
    // Skip when tmpdir itself is private to this user: nothing would be shared.
    for (let d = realpathSync(dir); d !== dirname(d); d = dirname(d)) {
      if ((statSync(dirname(d)).mode & 0o001) === 0) return;
    }
    const shared = join(dir, "lib", "node_modules", "locus-cli", "bin");
    const privateHome = join(dir, "root");
    mkdirSync(shared, { recursive: true, mode: 0o755 });
    mkdirSync(join(privateHome, ".locus", "bin"), { recursive: true, mode: 0o755 });
    chmodSync(privateHome, 0o700);
    const bin = join(privateHome, ".locus", "bin", "locus");
    writeFileSync(bin, "#!/bin/sh\necho native\n", { mode: 0o755 });
    const target = join(shared, "locus.js");
    writeFileSync(target, "// wrapper\n");
    assert.equal(linkNativeBin(bin, target, 0), false);
    assert.equal(lstatSync(target).isSymbolicLink(), false, "shared wrapper must stay in place");
    // A non-root install (the user's own prefix) still links.
    assert.equal(linkNativeBin(bin, target, 1000), true);
    assert.ok(lstatSync(target).isSymbolicLink());
    // As does a root install whose binary other users can reach.
    unlinkSync(target);
    writeFileSync(target, "// wrapper\n");
    chmodSync(privateHome, 0o755);
    assert.equal(linkNativeBin(bin, target, 0), true);
    assert.ok(lstatSync(target).isSymbolicLink());
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

// The release archive ships locus-mcp beside locus; npm users must get both.
test("locus: installCompanionBinaries installs locus-mcp next to the cached locus", () => {
  const { installCompanionBinaries } = require(wrappers.locus);
  const { existsSync, mkdirSync } = require("node:fs");
  const dir = mkdtempSync(join(tmpdir(), "locus-companion-"));
  try {
    const staging = join(dir, "staging", "locus-x86_64-unknown-linux-gnu");
    const cache = join(dir, "cache");
    mkdirSync(staging, { recursive: true });
    mkdirSync(cache);
    writeFileSync(join(staging, "locus"), "native");
    writeFileSync(join(staging, "locus-mcp"), "native-mcp");
    const installed = installCompanionBinaries(join(dir, "staging"), join(cache, "locus"), "linux");
    assert.deepEqual(installed, [join(cache, "locus-mcp")]);
    assert.equal(readFileSync(join(cache, "locus-mcp"), "utf8"), "native-mcp");
    assert.ok(existsSync(join(cache, "locus-mcp.version")));
    // An archive without locus-mcp is not an error.
    assert.deepEqual(installCompanionBinaries(join(dir, "empty"), join(cache, "locus"), "linux"), []);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("locus: wrapper drains unread HTTP responses and exits explicitly", () => {
  const src = readFileSync(wrappers.locus, "utf8");
  const download = src.split("function download(")[1].split("\nfunction ")[0];
  assert.equal((download.match(/res\.resume\(\)/g) || []).length, 2, "redirect and non-200 responses must be drained");
  assert.match(src.split("async function main()")[1], /process\.exit\(0\)/);
  assert.match(readFileSync(join(root, "npm", "install.js"), "utf8"), /process\.exit\(0\)/);
});
