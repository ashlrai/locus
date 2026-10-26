#!/usr/bin/env node

/**
 * Thin npm wrapper for the `locus` binary.
 *
 * 1. Prefer the newest compatible native binary in the cache, PATH or Cargo home
 * 2. Else download the matching GitHub release asset (locus-<target>.tar.gz)
 * 3. Else fall back to `cargo install --git` and use ~/.cargo/bin/locus
 */

const { execFileSync, spawnSync } = require("child_process");
const {
  chmodSync,
  closeSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readdirSync,
  readSync,
  rmSync,
  statSync,
  unlinkSync,
  writeFileSync,
} = require("fs");
const { delimiter, dirname, join } = require("path");
const https = require("https");
const crypto = require("crypto");

const VERSION = "0.5.2";
const REPO = "ashlrai/locus";
const BINARY_NAME = "locus";
const CARGO_PACKAGE = "locus-cli";
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
    `This can take several minutes and needs Rust (https://rustup.rs).`
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

// Compare SemVer precedence without numeric coercion (version components can
// exceed Number.MAX_SAFE_INTEGER). Build metadata does not affect precedence.
function parseVersion(value) {
  const match = value.trim().match(/^(?:locus\s+|v)?(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/);
  if (!match) return null;
  const prerelease = match[4] ? match[4].split(".") : [];
  if (prerelease.some((part) => /^0\d+$/.test(part))) return null;
  return { core: match.slice(1, 4), prerelease };
}

function compareNumeric(a, b) {
  return a.length === b.length ? (a > b ? 1 : a < b ? -1 : 0) : a.length > b.length ? 1 : -1;
}

function compareVersions(a, b) {
  for (let i = 0; i < 3; i++) {
    const compared = compareNumeric(a.core[i], b.core[i]);
    if (compared) return compared;
  }
  if (!a.prerelease.length || !b.prerelease.length) {
    return a.prerelease.length ? -1 : b.prerelease.length ? 1 : 0;
  }
  for (let i = 0; i < Math.max(a.prerelease.length, b.prerelease.length); i++) {
    const left = a.prerelease[i];
    const right = b.prerelease[i];
    if (left === undefined) return -1;
    if (right === undefined) return 1;
    if (left === right) continue;
    const leftNumeric = /^\d+$/.test(left);
    const rightNumeric = /^\d+$/.test(right);
    if (leftNumeric && rightNumeric) return compareNumeric(left, right);
    if (leftNumeric !== rightNumeric) return leftNumeric ? -1 : 1;
    return left > right ? 1 : -1;
  }
  return 0;
}

function binaryVersion(binaryPath) {
  if (isNodeShim(binaryPath)) return null;
  try {
    // Probe the executable itself: a sidecar can survive a manual upgrade.
    const out = execFileSync(binaryPath, ["--version"], {
      stdio: ["ignore", "pipe", "ignore"],
      timeout: 5_000,
      maxBuffer: 4096,
    });
    return parseVersion(out.toString());
  } catch {
    return null;
  }
}

function writeVersionSidecar(binaryPath) {
  try {
    writeFileSync(getVersionSidecarPath(binaryPath), `${VERSION}\n`, "utf8");
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

function extractBinaryFromArchive(archivePath, binaryPath) {
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

    copyFileSync(extractedBinaryPath, binaryPath);
    if (process.platform !== "win32") {
      chmodSync(binaryPath, 0o755);
    }
    writeVersionSidecar(binaryPath);
    installCompanionBinaries(stagingDir, binaryPath);
  } finally {
    rmSync(stagingDir, { recursive: true, force: true });
  }
}

// The release archive ships locus-mcp beside locus. Install it next to the
// cached locus so `locus setup` finds the sibling MCP server; without it npm
// users have no locus-mcp at all. Best-effort: never fails the locus install.
const COMPANION_BINARIES = Object.freeze(["locus-mcp"]);

function installCompanionBinaries(stagingDir, binaryPath, platform = process.platform) {
  const installed = [];
  const destDir = dirname(binaryPath);
  for (const name of COMPANION_BINARIES) {
    const filename = platform === "win32" ? `${name}.exe` : name;
    try {
      const src = findBinaryInDir(stagingDir, filename);
      if (!src) continue;
      const dest = join(destDir, filename);
      copyFileSync(src, dest);
      if (platform !== "win32") chmodSync(dest, 0o755);
      writeVersionSidecar(dest);
      installed.push(dest);
    } catch {
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
  const cargoHome = process.env.CARGO_HOME || join(home, ".cargo");
  return join(cargoHome, "bin", getBinaryFilename());
}

function isNodeShim(path) {
  try {
    const real = require("fs").realpathSync(path);
    if (real === require("fs").realpathSync(__filename)) return true;
    const fd = openSync(real, "r");
    try {
      const buffer = Buffer.alloc(64);
      const bytes = readSync(fd, buffer, 0, buffer.length, 0);
      const head = buffer.subarray(0, bytes).toString("utf8");
      return head.startsWith("#!") && head.includes("node");
    } finally {
      closeSync(fd);
    }
  } catch {
    return false;
  }
}

function findUsableBinary() {
  const filename = getBinaryFilename();
  const candidates = [getBinaryPath()];
  // Inspect every PATH entry, not just `which locus`: npm's own shim commonly
  // shadows a newer Homebrew binary. Never execute another Node wrapper.
  for (const dir of (process.env.PATH || "").split(delimiter).filter(Boolean)) {
    candidates.push(join(dir, filename));
  }
  candidates.push(cargoBinPath());
  let best = null;
  let bestVersion = parseVersion(VERSION);
  for (const candidate of new Set(candidates)) {
    if (!existsSync(candidate)) continue;
    const version = binaryVersion(candidate);
    if (version && compareVersions(version, bestVersion) >= 0) {
      // Keep the first candidate on equal precedence (prefer the cache).
      if (!best || compareVersions(version, bestVersion) > 0) {
        best = candidate;
        bestVersion = version;
      }
    }
  }
  return best;
}

function installViaCargo() {
  console.error(`Falling back to: ${INSTALL_FROM_SOURCE}`);
  const cargo = process.platform === "win32" ? "cargo.exe" : "cargo";
  const result = spawnSync(
    cargo,
    [
      "install",
      "--git",
      `https://github.com/${REPO}`,
      CARGO_PACKAGE,
      "--locked",
      // Let Cargo refuse an existing unmanaged binary. A failed version probe
      // must never turn into permission to overwrite a potentially newer CLI.
    ],
    { stdio: "inherit", env: process.env }
  );
  if (result.status !== 0) {
    throw new Error(
      `cargo install failed (exit ${result.status}). Install Rust from https://rustup.rs then retry, or run:\n  ${INSTALL_FROM_SOURCE}`
    );
  }
  const path = cargoBinPath();
  if (!existsSync(path)) {
    throw new Error(`cargo install succeeded but ${path} not found`);
  }
  return path;
}

async function downloadReleaseBinary(binaryPath) {
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
    extractBinaryFromArchive(archivePath, binaryPath);
    removeIfExists(archivePath);
    console.error(`Installed locus to ${binaryPath}`);
    return binaryPath;
  } catch (err) {
    removeIfExists(archivePath);
    throw err;
  }
}

async function ensureBinary() {
  const binaryPath = getBinaryPath();

  const existing = findUsableBinary();
  if (existing) return existing;
  // Keep an older cache intact until a replacement has been downloaded and
  // extracted successfully; a network failure must not destroy an install.

  let target = null;
  try {
    target = getPlatformTarget();
  } catch {
    // Unsupported platform: downloadReleaseBinary reports it, then cargo runs.
  }
  if (target && !hasPrebuiltBinary(target)) {
    console.error(cargoFallbackMessage(process, "no-prebuilt"));
    try {
      return installViaCargo();
    } catch (cargoErr) {
      console.error(cargoErr.message);
      process.exit(1);
    }
  }

  try {
    return await downloadReleaseBinary(binaryPath);
  } catch (err) {
    console.error(`Failed to download locus: ${err.message}`);
    console.error(cargoFallbackMessage(process, "download-failed"));
    try {
      return installViaCargo();
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
  const binary = await ensureBinary();
  const args = process.argv.slice(2);
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
  main();
}

module.exports = {
  getBinaryPath,
  findUsableBinary,
  parseVersion,
  compareVersions,
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
