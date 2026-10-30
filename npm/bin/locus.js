#!/usr/bin/env node

/**
 * Thin npm wrapper for the `locus` binary.
 *
 * 1. Prefer a cached download under ~/.locus/bin
 * 2. Else download the matching GitHub release asset (locus-<target>.tar.gz)
 * 3. Else build with cargo in an isolated install root
 */

const { execFileSync, spawnSync } = require("child_process");
const {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
  unlinkSync,
  writeFileSync,
} = require("fs");
const { dirname, join } = require("path");
const https = require("https");
const crypto = require("crypto");

const VERSION = "0.5.2";
const REPO = "ashlrai/locus";
const BINARY_NAME = "locus";
const CARGO_PACKAGE = "locus-cli";
// Minimum Rust for the cargo fallback; must equal the workspace `rust-version`
// in the repo's Cargo.toml (crates/locus-core/tests/rust_version.rs checks it).
const MIN_RUST_VERSION = "1.89";
// `cargo install` takes the crate as a positional argument; it has no --package flag.
const INSTALL_FROM_SOURCE = `cargo install --git https://github.com/${REPO} ${CARGO_PACKAGE} --locked`;
const CACHE_DIR = join(
  process.env.HOME || process.env.USERPROFILE || "/tmp",
  ".locus",
  "bin"
);

const SUPPORTED_TARGETS = Object.freeze({
  "darwin-arm64": "aarch64-apple-darwin",
  "darwin-x64": "x86_64-apple-darwin",
  "linux-arm64": "aarch64-unknown-linux-gnu",
  "linux-x64": "x86_64-unknown-linux-gnu",
  "win32-arm64": "aarch64-pc-windows-msvc",
  "win32-x64": "x86_64-pc-windows-msvc",
});

// Targets release.yml actually builds and uploads. The other SUPPORTED_TARGETS
// have no prebuilt asset yet, so they go straight to the cargo fallback.
// Keep in sync with the matrix in .github/workflows/release.yml.
const PREBUILT_TARGETS = Object.freeze([
  "aarch64-apple-darwin",
  "x86_64-apple-darwin",
  "x86_64-unknown-linux-gnu",
]);

function hasPrebuiltBinary(target) {
  return PREBUILT_TARGETS.includes(target);
}

function cargoFallbackMessage(runtime = process, reason = "no-prebuilt") {
  const platform = `${runtime.platform}-${runtime.arch}`;
  const why =
    reason === "no-prebuilt"
      ? `No prebuilt locus binary for ${platform} in v${VERSION}`
      : `Could not download the prebuilt locus v${VERSION} binary for ${platform}`;
  return (
    `${why}; building from source with cargo instead. ` +
    `This can take several minutes and needs Rust ${MIN_RUST_VERSION} or newer (https://rustup.rs).`
  );
}

function unsupportedPlatformMessage(runtime = process) {
  return `Unsupported platform: ${runtime.platform}-${runtime.arch}. Install from source: ${INSTALL_FROM_SOURCE}`;
}

function getPlatformTarget(runtime = process) {
  const target = SUPPORTED_TARGETS[`${runtime.platform}-${runtime.arch}`];
  if (!target) {
    throw new Error(unsupportedPlatformMessage(runtime));
  }
  return target;
}

function getBinaryFilename() {
  return process.platform === "win32" ? `${BINARY_NAME}.exe` : BINARY_NAME;
}

function getBinaryPath() {
  return join(CACHE_DIR, getBinaryFilename());
}

function getVersionSidecarPath(binaryPath) {
  return `${binaryPath}.version`;
}

function download(url) {
  return new Promise((resolve, reject) => {
    https
      .get(url, (res) => {
        if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
          // Drain every response we do not read, or its socket keeps the
          // process alive after the download finishes.
          res.resume();
          return download(res.headers.location).then(resolve).catch(reject);
        }
        if (res.statusCode !== 200) {
          res.resume();
          return reject(new Error(`HTTP ${res.statusCode} for ${url}`));
        }
        const chunks = [];
        res.on("data", (chunk) => chunks.push(chunk));
        res.on("end", () => resolve(Buffer.concat(chunks)));
        res.on("error", reject);
      })
      .on("error", reject);
  });
}

function parseSha256File(buf, expectedFilename) {
  // Standard sha256sum format: "<64-hex>  <filename>\n" (may be multi-line)
  const lines = buf.toString("utf8").trim().split(/\r?\n/);
  for (const line of lines) {
    const m = line.match(/^([0-9a-f]{64})\s+\*?(.+)$/i);
    if (!m) continue;
    if (!expectedFilename || m[2].trim() === expectedFilename) {
      return m[1].toLowerCase();
    }
  }
  return null;
}

function parseVersion(value) {
  const match = value.trim().match(
    /^(?:locus(?:-mcp)?\s+)?v?(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/
  );
  if (!match) return null;
  const prerelease = match[4] ? match[4].split(".") : [];
  if (prerelease.some((part) => /^0\d+$/.test(part))) return null;
  return { core: match.slice(1, 4).map(BigInt), prerelease };
}

// SemVer precedence: numeric components, prereleases before stable releases,
// numeric prerelease identifiers before text, and no build-metadata ordering.
function compareVersions(left, right) {
  const a = parseVersion(left);
  const b = parseVersion(right);
  if (!a || !b) return null;
  for (let i = 0; i < 3; i++) {
    if (a.core[i] !== b.core[i]) return a.core[i] > b.core[i] ? 1 : -1;
  }
  if (!a.prerelease.length || !b.prerelease.length) {
    return a.prerelease.length === b.prerelease.length ? 0 : a.prerelease.length ? -1 : 1;
  }
  for (let i = 0; i < Math.max(a.prerelease.length, b.prerelease.length); i++) {
    const x = a.prerelease[i];
    const y = b.prerelease[i];
    if (x === undefined) return -1;
    if (y === undefined) return 1;
    if (x === y) continue;
    const xNumeric = /^\d+$/.test(x);
    const yNumeric = /^\d+$/.test(y);
    if (xNumeric && yNumeric) return BigInt(x) > BigInt(y) ? 1 : -1;
    if (xNumeric !== yNumeric) return xNumeric ? -1 : 1;
    return x > y ? 1 : -1;
  }
  return 0;
}

function readBinaryVersion(binaryPath) {
  // Probe the executable first: another installer may have replaced it without
  // updating our sidecar. Never probe a Node shim (it could recurse into us).
  try {
    if (!isNodeShim(binaryPath)) {
      const out = execFileSync(binaryPath, ["--version"], {
        stdio: ["ignore", "pipe", "ignore"],
        timeout: 5000,
      }).toString().trim();
      if (parseVersion(out)) return out.replace(/^(?:locus(?:-mcp)?\s+)?v?/, "");
    }
  } catch {}
  try {
    const version = readFileSync(getVersionSidecarPath(binaryPath), "utf8").trim();
    if (parseVersion(version)) return version.replace(/^(?:locus(?:-mcp)?\s+)?v?/, "");
  } catch {}
  return null;
}

function isCachedBinaryStale(binaryPath) {
  const version = readBinaryVersion(binaryPath);
  return version === null || compareVersions(version, VERSION) !== 0;
}

function assertCanReplace(binaryPath, { force = false } = {}, replacementVersion = VERSION) {
  if (force || !existsSync(binaryPath)) return;
  const version = readBinaryVersion(binaryPath);
  if (version === null || replacementVersion === null || compareVersions(version, replacementVersion) > 0) {
    const reason = version === null
      ? "its installed version could not be determined"
      : replacementVersion === null
        ? "the replacement version could not be determined"
        : `installed v${version} is newer than ${replacementVersion === VERSION ? "npm's" : "the replacement's"} v${replacementVersion}`;
    const error = new Error(
      `Refusing to replace ${binaryPath}: ${reason}. ` +
      "Kept the installed binary. To allow replacement, pass --force before the locus command (or run node install.js --force in the npm package)."
    );
    error.code = "LOCUS_NPM_DOWNGRADE";
    throw error;
  }
}

function writeVersionSidecar(binaryPath, version = VERSION) {
  try {
    if (version === null) unlinkSync(getVersionSidecarPath(binaryPath));
    else writeFileSync(getVersionSidecarPath(binaryPath), `${version}\n`, "utf8");
  } catch {
    // best-effort
  }
}

function findBinaryInDir(dir, binaryFilename) {
  const direct = join(dir, binaryFilename);
  if (existsSync(direct) && statSync(direct).isFile()) {
    return direct;
  }
  // release.yml packs locus-<target>/{locus,locus-mcp}
  let entries;
  try {
    entries = readdirSync(dir);
  } catch {
    return null;
  }
  for (const name of entries) {
    const child = join(dir, name);
    try {
      if (!statSync(child).isDirectory()) continue;
    } catch {
      continue;
    }
    const nested = join(child, binaryFilename);
    if (existsSync(nested) && statSync(nested).isFile()) {
      return nested;
    }
  }
  return null;
}

function extractBinaryFromArchive(archivePath, binaryPath, options) {
  const stagingDir = mkdtempSync(join(CACHE_DIR, ".extract-"));
  const binaryFilename = getBinaryFilename();

  try {
    if (process.platform === "win32") {
      execFileSync(
        "powershell",
        [
          "-NoProfile",
          "-NonInteractive",
          "-Command",
          "& { param($archive, $destination) Expand-Archive -LiteralPath $archive -DestinationPath $destination -Force }",
          archivePath,
          stagingDir,
        ],
        { stdio: "pipe" }
      );
    } else {
      execFileSync("tar", ["xzf", archivePath, "-C", stagingDir], { stdio: "pipe" });
    }

    const extractedBinaryPath = findBinaryInDir(stagingDir, binaryFilename);
    if (!extractedBinaryPath) {
      throw new Error(`${binaryFilename} not found in ${archivePath}`);
    }

    // Recheck after the download in case another installer updated the cache.
    assertCanReplace(binaryPath, options);
    copyFileSync(extractedBinaryPath, binaryPath);
    if (process.platform !== "win32") {
      chmodSync(binaryPath, 0o755);
    }
    writeVersionSidecar(binaryPath);
    installCompanionBinaries(stagingDir, binaryPath, process.platform, options);
  } finally {
    rmSync(stagingDir, { recursive: true, force: true });
  }
}

// The release archive ships locus-mcp beside locus. Install it next to the
// cached locus so `locus setup` finds the sibling MCP server; without it npm
// users have no locus-mcp at all. Best-effort: never fails the locus install.
const COMPANION_BINARIES = Object.freeze(["locus-mcp"]);

function installCompanionBinaries(stagingDir, binaryPath, platform = process.platform, options) {
  const installed = [];
  const destDir = dirname(binaryPath);
  for (const name of COMPANION_BINARIES) {
    const filename = platform === "win32" ? `${name}.exe` : name;
    try {
      const src = findBinaryInDir(stagingDir, filename);
      if (!src) continue;
      const dest = join(destDir, filename);
      assertCanReplace(dest, options);
      copyFileSync(src, dest);
      if (platform !== "win32") chmodSync(dest, 0o755);
      writeVersionSidecar(dest);
      installed.push(dest);
    } catch (err) {
      if (err.code === "LOCUS_NPM_DOWNGRADE") console.error(err.message);
      // best-effort
    }
  }
  return installed;
}

function removeIfExists(path) {
  try {
    unlinkSync(path);
  } catch {
    // best-effort cleanup only
  }
}

function cargoBinPath() {
  const home = process.env.HOME || process.env.USERPROFILE || "/tmp";
  const name = getBinaryFilename();
  return join(home, ".cargo", "bin", name);
}

function isNodeShim(path) {
  try {
    const real = require("fs").realpathSync(path);
    if (real === require("fs").realpathSync(__filename)) return true;
    const head = readFileSync(real).subarray(0, 64).toString("utf8");
    return head.startsWith("#!") && head.includes("node");
  } catch {
    return false;
  }
}

function tryExistingOnPath() {
  const which = process.platform === "win32" ? "where" : "which";
  const result = spawnSync(which, [BINARY_NAME], { encoding: "utf8" });
  if (result.status === 0) {
    // Skip this npm shim itself (and any other node wrapper): `which` finds
    // the npm-installed bin first, and executing it recurses forever.
    const candidates = (result.stdout || "").split(/\r?\n/).map((s) => s.trim()).filter(Boolean);
    for (const path of candidates) {
      if (path && existsSync(path) && !isNodeShim(path)) {
        return path;
      }
    }
  }
  const cargoPath = cargoBinPath();
  if (existsSync(cargoPath)) {
    return cargoPath;
  }
  return null;
}

function installViaCargo(binaryPath, options) {
  console.error(`Falling back to: ${INSTALL_FROM_SOURCE}`);
  const cargo = process.platform === "win32" ? "cargo.exe" : "cargo";
  mkdirSync(CACHE_DIR, { recursive: true });
  const root = mkdtempSync(join(CACHE_DIR, ".cargo-install-"));
  try {
    const result = spawnSync(
      cargo,
      [
        "install",
        "--git",
        `https://github.com/${REPO}`,
        CARGO_PACKAGE,
        "--locked",
        "--root",
        root,
      ],
      { stdio: "inherit", env: process.env }
    );
    if (result.status !== 0) {
      throw new Error(
        `cargo install failed (exit ${result.status}). Install Rust ${MIN_RUST_VERSION} or newer from https://rustup.rs then retry, or run:\n  ${INSTALL_FROM_SOURCE}`
      );
    }
    const path = join(root, "bin", getBinaryFilename());
    if (!existsSync(path)) {
      throw new Error(`cargo install succeeded but ${path} not found`);
    }
    const version = readBinaryVersion(path);
    assertCanReplace(binaryPath, options, version);
    copyFileSync(path, binaryPath);
    if (process.platform !== "win32") chmodSync(binaryPath, 0o755);
    writeVersionSidecar(binaryPath, version);
    return binaryPath;
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

async function downloadReleaseBinary(binaryPath, options) {
  assertCanReplace(binaryPath, options);
  const target = getPlatformTarget();
  const isWindows = process.platform === "win32";
  const archiveExt = isWindows ? "zip" : "tar.gz";
  // Matches .github/workflows/release.yml: locus-<target>.tar.gz
  const archiveName = `locus-${target}.${archiveExt}`;
  const url = `https://github.com/${REPO}/releases/download/v${VERSION}/${archiveName}`;
  const sha256Url = `${url}.sha256`;

  console.error(`Downloading locus v${VERSION} for ${target}...`);
  mkdirSync(CACHE_DIR, { recursive: true });

  const archivePath = join(CACHE_DIR, archiveName);

  try {
    // Checksum is optional until release.yml publishes .sha256 sidecars
    let expected = null;
    try {
      const sumBuf = await download(sha256Url);
      expected = parseSha256File(sumBuf, archiveName);
    } catch {
      expected = null;
    }

    const data = await download(url);

    if (expected) {
      const actual = crypto.createHash("sha256").update(data).digest("hex");
      const expectedBuf = Buffer.from(expected, "hex");
      const actualBuf = Buffer.from(actual, "hex");
      if (
        expectedBuf.length !== actualBuf.length ||
        !crypto.timingSafeEqual(expectedBuf, actualBuf)
      ) {
        throw new Error(
          `SHA-256 mismatch for ${archiveName}: expected ${expected}, got ${actual}`
        );
      }
    } else {
      console.error(
        `Warning: no checksum at ${sha256Url} — installing without verification`
      );
    }

    writeFileSync(archivePath, data);
    extractBinaryFromArchive(archivePath, binaryPath, options);
    removeIfExists(archivePath);
    console.error(`Installed locus to ${binaryPath}`);
    return binaryPath;
  } catch (err) {
    removeIfExists(archivePath);
    throw err;
  }
}

async function ensureBinary(options = {}) {
  const binaryPath = getBinaryPath();

  if (existsSync(binaryPath)) {
    assertCanReplace(binaryPath, options);
    if (!isCachedBinaryStale(binaryPath)) return binaryPath;
    console.error(`Cached locus is out of date — refreshing to v${VERSION}...`);
  }

  // Prefer already-installed binary (brew / prior cargo install)
  const existing = tryExistingOnPath();
  if (existing) {
    const version = readBinaryVersion(existing);
    const comparison = version === null ? null : compareVersions(version, VERSION);
    // Reuse newer PATH installs, unless the user explicitly requested replacement.
    if (comparison === 0 || (comparison > 0 && !options.force)) return existing;
  }

  let target = null;
  try {
    target = getPlatformTarget();
  } catch {
    // Unsupported platform: downloadReleaseBinary reports it, then cargo runs.
  }
  if (target && !hasPrebuiltBinary(target)) {
    console.error(cargoFallbackMessage(process, "no-prebuilt"));
    try {
      return installViaCargo(binaryPath, options);
    } catch (cargoErr) {
      console.error(cargoErr.message);
      process.exit(1);
    }
  }

  try {
    return await downloadReleaseBinary(binaryPath, options);
  } catch (err) {
    // A refusal must not turn into a cargo install that bypasses the guard.
    if (err.code === "LOCUS_NPM_DOWNGRADE") throw err;
    console.error(`Failed to download locus: ${err.message}`);
    console.error(cargoFallbackMessage(process, "download-failed"));
    try {
      return installViaCargo(binaryPath, options);
    } catch (cargoErr) {
      console.error(cargoErr.message);
      process.exit(1);
    }
  }
}

// Session authority is anchored to the *parent* process of `locus` (the user's
// shell). When this Node wrapper sits in between, the anchor binds to a
// short-lived node process, so `locus quickstart` then `locus enter <alias>`
// fails with "session authority generation is stale". On POSIX we therefore
// replace this wrapper with a symlink to the native binary, so the shell runs
// locus directly. Best-effort: on any failure the wrapper keeps working.
const AUTHORITY_COMMANDS = new Set(["quickstart", "pin", "enter", "init", "leave"]);

// True when a user other than the owner (no shared group assumed) can reach
// `path`: it and every ancestor directory are world-traversable (for a file:
// world-executable).
function reachableByOthers(path) {
  const fs = require("fs");
  try {
    let current = fs.realpathSync(path);
    if ((fs.statSync(current).mode & 0o001) === 0) return false;
    for (;;) {
      const parent = dirname(current);
      if (parent === current) return true;
      if ((fs.statSync(parent).mode & 0o001) === 0) return false;
      current = parent;
    }
  } catch {
    return false;
  }
}

function linkNativeBin(
  binary,
  target = __filename,
  uid = typeof process.getuid === "function" ? process.getuid() : -1
) {
  if (process.platform === "win32" || process.env.LOCUS_NPM_NO_LINK) return false;
  const fs = require("fs");
  try {
    if (fs.lstatSync(target).isSymbolicLink()) return false;
    // A root install into a shared prefix (e.g. `sudo npm install -g` into
    // /usr/lib/node_modules) must not point its bin into root's private HOME
    // (/root is 0700): other users would get "Permission denied". Keep the
    // wrapper there; each user's run then resolves a binary under their own
    // HOME. Non-root installs still link (e.g. Homebrew node on macOS, whose
    // prefix is world-readable while HOME is 0750), as they are the user's own.
    if (uid === 0 && reachableByOthers(dirname(target)) && !reachableByOthers(binary)) {
      return false;
    }
    const tmp = `${target}.${process.pid}.tmp`;
    removeIfExists(tmp);
    fs.symlinkSync(binary, tmp);
    fs.renameSync(tmp, target);
    return true;
  } catch {
    return false;
  }
}

async function main() {
  const args = process.argv.slice(2);
  // Only a leading wrapper flag authorizes replacement. Subcommand flags such
  // as `leave --force` must reach the native CLI without authorizing a downgrade.
  const force = args[0] === "--force";
  if (force) args.shift();
  const binary = await ensureBinary({ force });
  if (linkNativeBin(binary) && AUTHORITY_COMMANDS.has(args[0])) {
    console.error(
      "locus: linked the native binary for future runs. This first run went through the npm wrapper; if a later command reports a stale session authority, re-run `locus " + args.join(" ") + "`."
    );
  }

  try {
    execFileSync(binary, args, { stdio: "inherit" });
  } catch (err) {
    process.exit(err.status || 1);
  }
  // Exit explicitly: a lingering download socket must never hold the shell.
  process.exit(0);
}

if (require.main === module) {
  main().catch((err) => {
    console.error(err.message);
    process.exit(1);
  });
}

module.exports = {
  getBinaryPath,
  isCachedBinaryStale,
  compareVersions,
  assertCanReplace,
  downloadReleaseBinary,
  ensureBinary,
  linkNativeBin,
  reachableByOthers,
  INSTALL_FROM_SOURCE,
  PREBUILT_TARGETS,
  SUPPORTED_TARGETS,
  cargoFallbackMessage,
  hasPrebuiltBinary,
  getPlatformTarget,
  parseSha256File,
  findBinaryInDir,
  installCompanionBinaries,
};
