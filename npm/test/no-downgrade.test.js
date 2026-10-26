// Issue #177: npm must preserve a newer native install, including installs
// hidden behind its own PATH shim. Every invocation stays offline in a HOME
// owned by this test; any attempted download or Cargo install fails the test.
const assert = require("node:assert/strict");
const { spawnSync } = require("node:child_process");
const { chmodSync, copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } = require("node:fs");
const { tmpdir } = require("node:os");
const { delimiter, join } = require("node:path");
const test = require("node:test");
const { parseVersion, compareVersions } = require("../bin/locus.js");

function fixture(run) {
  const home = mkdtempSync(join(tmpdir(), "locus-no-downgrade-"));
  try {
    const pkg = join(home, "package");
    mkdirSync(join(pkg, "bin"), { recursive: true });
    for (const file of ["bin/locus.js", "install.js"]) {
      copyFileSync(join(__dirname, "..", file), join(pkg, file));
    }
    chmodSync(join(pkg, "bin/locus.js"), 0o755);
    const shimBin = join(home, "shim-bin");
    const nativeBin = join(home, "native-bin");
    const cargoHome = join(home, "cargo-home");
    for (const dir of [shimBin, nativeBin, join(cargoHome, "bin")]) mkdirSync(dir, { recursive: true });
    symlinkSync(join(pkg, "bin/locus.js"), join(shimBin, "locus"));
    writeFileSync(join(shimBin, "cargo"), "#!/bin/sh\necho 'UNEXPECTED_CARGO' >&2\nexit 99\n", { mode: 0o755 });
    const offline = join(home, "offline.js");
    writeFileSync(offline, 'require("https").get = () => { throw new Error("UNEXPECTED_DOWNLOAD"); };\n');
    const env = {
      HOME: home,
      USERPROFILE: home,
      CARGO_HOME: cargoHome,
      PATH: [shimBin, nativeBin].join(delimiter),
      LOCUS_NPM_NO_LINK: "1",
    };
    function binary(path, version) {
      mkdirSync(require("node:path").dirname(path), { recursive: true });
      writeFileSync(path, `#!/bin/sh\nif [ "$1" = "--version" ]; then echo 'locus ${version}'; else echo 'selected ${version}'; fi\n`, { mode: 0o755 });
      return path;
    }
    function invoke(postinstall = false) {
      const commandEnv = { ...env };
      if (postinstall) delete commandEnv.LOCUS_NPM_NO_LINK;
      const result = spawnSync(process.execPath, ["--require", offline, join(pkg, postinstall ? "install.js" : "bin/locus.js"), "whoami"], {
        env: commandEnv,
        encoding: "utf8",
        timeout: 15_000,
      });
      assert.equal(result.status, 0, result.stderr);
      assert.doesNotMatch(result.stderr + result.stdout, /UNEXPECTED_(DOWNLOAD|CARGO)/);
      return result;
    }
    run({ home, pkg, shimBin, nativeBin, cargoHome, offline, env, binary, invoke });
  } finally {
    rmSync(home, { recursive: true, force: true });
  }
}

test("#177: newer cached locus survives an older npm wrapper and stale sidecar", { skip: process.platform === "win32" }, () => fixture(({ home, binary, invoke }) => {
  const cached = binary(join(home, ".locus/bin/locus"), "0.6.0");
  writeFileSync(`${cached}.version`, "0.5.1\n");
  assert.equal(invoke().stdout.trim(), "selected 0.6.0");
  assert.match(readFileSync(cached, "utf8"), /0\.6\.0/);
  assert.equal(readFileSync(`${cached}.version`, "utf8"), "0.5.1\n");
}));

test("#177: newest native PATH binary wins over cache and npm's shadowing shim", { skip: process.platform === "win32" }, () => fixture(({ home, nativeBin, binary, invoke }) => {
  binary(join(home, ".locus/bin/locus"), "0.5.2");
  binary(join(nativeBin, "locus"), "0.7.0");
  assert.equal(invoke().stdout.trim(), "selected 0.7.0");
}));

test("#177: custom Cargo home newer binary wins over a newer cached binary", { skip: process.platform === "win32" }, () => fixture(({ home, cargoHome, binary, invoke }) => {
  binary(join(home, ".locus/bin/locus"), "0.6.0");
  binary(join(cargoHome, "bin/locus"), "0.10.0-rc.2");
  assert.equal(invoke().stdout.trim(), "selected 0.10.0-rc.2");
}));

test("#177: postinstall links an existing newer native binary without downloading", { skip: process.platform === "win32" }, () => fixture(({ nativeBin, binary, invoke }) => {
  const native = binary(join(nativeBin, "locus"), "1.0.0");
  assert.match(invoke(true).stdout, new RegExp(`linked native binary ${native}`));
}));

test("#177: version comparison follows SemVer precedence rather than substring matching", () => {
  const baseline = parseVersion("locus 0.5.2");
  for (const value of ["0.5.2", "0.5.2+local", "0.5.20", "0.6.0-rc.1", "10.0.0"]) {
    assert.ok(compareVersions(parseVersion(value), baseline) >= 0, value);
  }
  for (const value of ["0.5.1", "0.5.2-rc.1", "0.5.2-1", "0.4.99"]) {
    assert.ok(compareVersions(parseVersion(value), baseline) < 0, value);
  }
  assert.ok(compareVersions(parseVersion("0.6.0-rc.10"), parseVersion("0.6.0-rc.2")) > 0);
  assert.ok(compareVersions(parseVersion("0.6.0-1"), parseVersion("0.6.0-alpha")) < 0);
  assert.ok(compareVersions(parseVersion("0.6.0-alpha.1"), parseVersion("0.6.0-alpha")) > 0);
  for (const value of ["garbage 0.5.2", "locus 0.5.2 trailing", "0.05.2", "0.5.2-01", ""]) {
    assert.equal(parseVersion(value), null, value);
  }
});


test("#177: Cargo fallback refuses to overwrite an installed binary with an unknown version", { skip: process.platform === "win32" }, () => fixture(({ home, pkg, shimBin, cargoHome, offline, env, binary }) => {
  const native = binary(join(cargoHome, "bin/locus"), "development");
  const before = readFileSync(native, "utf8");
  const args = join(home, "cargo-args");
  writeFileSync(join(shimBin, "cargo"), `#!/bin/sh
printf '%s\n' "$@" > "${args}"
for arg in "$@"; do
  if [ "$arg" = "--force" ]; then
    printf 'overwritten' > "${native}"
    exit 0
  fi
done
echo 'refusing existing binary' >&2
exit 17
`, { mode: 0o755 });
  // No asset exists for this target, so this exercises the actual fallback
  // without any network request or invocation of the real toolchain.
  writeFileSync(offline, 'require("https").get = () => { throw new Error("UNEXPECTED_DOWNLOAD"); }; Object.defineProperty(process, "platform", { value: "linux" }); Object.defineProperty(process, "arch", { value: "arm64" });\n');
  const result = spawnSync(process.execPath, ["--require", offline, join(pkg, "bin/locus.js"), "whoami"], { env, encoding: "utf8", timeout: 15_000 });
  assert.equal(result.status, 1, result.stderr);
  assert.match(result.stderr, /cargo install failed \(exit 17\)/);
  assert.doesNotMatch(result.stderr, /UNEXPECTED_DOWNLOAD/);
  assert.doesNotMatch(readFileSync(args, "utf8"), /--force/);
  assert.equal(readFileSync(native, "utf8"), before);
}));

test("#177: an older cached native can upgrade through Cargo without forced replacement", { skip: process.platform === "win32" }, () => fixture(({ home, pkg, shimBin, cargoHome, offline, env, binary }) => {
  const cached = binary(join(home, ".locus/bin/locus"), "0.5.1");
  const before = readFileSync(cached, "utf8");
  const source = binary(join(home, "replacement"), "0.5.2");
  const native = join(cargoHome, "bin/locus");
  writeFileSync(join(shimBin, "cargo"), `#!/bin/sh
for arg in "$@"; do
  if [ "$arg" = "--force" ]; then exit 99; fi
done
/bin/cp "${source}" "${native}"
`, { mode: 0o755 });
  writeFileSync(offline, 'require("https").get = () => { throw new Error("UNEXPECTED_DOWNLOAD"); }; Object.defineProperty(process, "platform", { value: "linux" }); Object.defineProperty(process, "arch", { value: "arm64" });\n');
  const result = spawnSync(process.execPath, ["--require", offline, join(pkg, "bin/locus.js"), "whoami"], { env, encoding: "utf8", timeout: 15_000 });
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout.trim(), "selected 0.5.2");
  assert.doesNotMatch(result.stderr, /UNEXPECTED_DOWNLOAD/);
  assert.equal(readFileSync(cached, "utf8"), before, "the old cache survives the source fallback");
}));
