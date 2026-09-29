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
  return [...yml.matchAll(/^\s*-\s*target:\s*([\w-]+)\s*$/gm)].map((m) => m[1]).sort();
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
