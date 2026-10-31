#!/usr/bin/env node

/**
 * Thin npm wrapper (published as `@ashlrai/locus-mcp`) for the `locus-mcp` binary.
 *
 * 1. Prefer a cached download under ~/.locus/bin
 * 2. Else download the matching GitHub release asset (locus-<target>.tar.gz)
 * 3. Else build with cargo in an isolated install root
 *
 * The wrapper never replaces a newer cached binary with its older release:
 * a cached locus-mcp that reports (or whose sidecar records) a newer version
 * is kept and used. A leading `--force` is the explicit override.
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
const { join } = require("path");
const https = require("https");
const crypto = require("crypto");

const VERSION = "0.6.0";
const REPO = "ashlrai/locus";
const BINARY_NAME = "locus-mcp";
const CARGO_PACKAGE = "locus-mcp";
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
      ? `No prebuilt locus-mcp binary for ${platform} in v${VERSION}`
      : `Could not download the prebuilt locus-mcp v${VERSION} binary for ${platform}`;
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
  // Only a provably older cache is stale. A newer or unversioned binary is
  // reused as is so a refresh can never downgrade an existing install.
  const version = readBinaryVersion(binaryPath);
  return version !== null && compareVersions(version, VERSION) < 0;
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
      "Kept the installed binary. To allow replacement, pass --force before the locus-mcp command."
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
  } finally {
    rmSync(stagingDir, { recursive: true, force: true });
  }
}

function removeIfExists(path) {
  try {
    unlinkSync(path);
  } catch {
    // best-effort
  }
}

function cargoBinPath() {
  const home = process.env.HOME || process.env.USERPROFILE || "/tmp";
  return join(home, ".cargo", "bin", getBinaryFilename());
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
  const archiveName = `locus-${target}.${archiveExt}`;
  const url = `https://github.com/${REPO}/releases/download/v${VERSION}/${archiveName}`;
  const sha256Url = `${url}.sha256`;

  console.error(`Downloading locus-mcp v${VERSION} for ${target}...`);
  mkdirSync(CACHE_DIR, { recursive: true });

  const archivePath = join(CACHE_DIR, archiveName);

  try {
    let expected;
    try {
      const sumBuf = await download(sha256Url);
      expected = parseSha256File(sumBuf, archiveName);
    } catch (err) {
      const checksumErr = new Error(
        `Could not download SHA-256 checksum from ${sha256Url}: ${err.message}. Check your connection and retry.`
      );
      checksumErr.code = "LOCUS_NPM_CHECKSUM";
      throw checksumErr;
    }
    if (!expected) {
      const err = new Error(
        `Invalid or missing SHA-256 checksum for ${archiveName} at ${sha256Url}. Check the release sidecar and retry.`
      );
      err.code = "LOCUS_NPM_CHECKSUM";
      throw err;
    }

    const data = await download(url);
    const actual = crypto.createHash("sha256").update(data).digest("hex");
    const expectedBuf = Buffer.from(expected, "hex");
    const actualBuf = Buffer.from(actual, "hex");
    if (
      expectedBuf.length !== actualBuf.length ||
      !crypto.timingSafeEqual(expectedBuf, actualBuf)
    ) {
      const err = new Error(
        `SHA-256 mismatch for ${archiveName}: expected ${expected}, got ${actual}. Check the release assets and retry.`
      );
      err.code = "LOCUS_NPM_CHECKSUM";
      throw err;
    }

    writeFileSync(archivePath, data);
    extractBinaryFromArchive(archivePath, binaryPath, options);
    removeIfExists(archivePath);
    console.error(`Installed locus-mcp to ${binaryPath}`);
    return binaryPath;
  } catch (err) {
    removeIfExists(archivePath);
    throw err;
  }
}

async function ensureBinary(options = {}) {
  const binaryPath = getBinaryPath();

  if (existsSync(binaryPath)) {
    const version = readBinaryVersion(binaryPath);
    const comparison = version === null ? null : compareVersions(version, VERSION);
    if (!options.force && (comparison === null || comparison >= 0)) {
      // A newer or unversioned cache is kept and run: using it is not a
      // downgrade, and replacing it would be.
      if (comparison > 0) {
        console.error(
          `Cached locus-mcp v${version} is newer than npm's v${VERSION}; keeping it.`
        );
      }
      return binaryPath;
    }
    console.error(
      comparison > 0
        ? `Replacing cached locus-mcp v${version} with npm's v${VERSION} (--force).`
        : `Cached locus-mcp is out of date; refreshing to v${VERSION}...`
    );
  }

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
    // A refusal or failed integrity check must not fall through to cargo.
    if (err.code === "LOCUS_NPM_DOWNGRADE" || err.code === "LOCUS_NPM_CHECKSUM") throw err;
    console.error(`Failed to download locus-mcp: ${err.message}`);
    console.error(cargoFallbackMessage(process, "download-failed"));
    try {
      return installViaCargo(binaryPath, options);
    } catch (cargoErr) {
      console.error(cargoErr.message);
      process.exit(1);
    }
  }
}

async function main() {
  const args = process.argv.slice(2);
  // Only a leading wrapper flag authorizes replacement; it is consumed here
  // and never reaches the native server.
  const force = args[0] === "--force";
  if (force) args.shift();
  const binary = await ensureBinary({ force });

  try {
    execFileSync(binary, args, { stdio: "inherit" });
  } catch (err) {
    process.exit(err.status || 1);
  }
  // Exit explicitly: a lingering download socket must never hold the client.
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
  readBinaryVersion,
  assertCanReplace,
  downloadReleaseBinary,
  ensureBinary,
  INSTALL_FROM_SOURCE,
  PREBUILT_TARGETS,
  SUPPORTED_TARGETS,
  cargoFallbackMessage,
  hasPrebuiltBinary,
  getPlatformTarget,
  parseSha256File,
  findBinaryInDir,
};
