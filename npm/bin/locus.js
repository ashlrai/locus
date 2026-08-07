#!/usr/bin/env node

/**
 * Thin npm wrapper for the `locus` binary.
 *
 * 1. Prefer a cached download under ~/.locus/bin
 * 2. Else download the matching GitHub release asset (locus-<target>.tar.gz)
 * 3. Else fall back to `cargo install --git` and use ~/.cargo/bin/locus
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

const VERSION = "0.1.0";
const REPO = "ashlrai/locus";
const BINARY_NAME = "locus";
const CARGO_PACKAGE = "locus-cli";
const INSTALL_FROM_SOURCE = `cargo install --git https://github.com/${REPO} --package ${CARGO_PACKAGE} --locked`;
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
          return download(res.headers.location).then(resolve).catch(reject);
        }
        if (res.statusCode !== 200) {
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

function isCachedBinaryStale(binaryPath) {
  // Prefer sidecar (locus-mcp has no --version CLI; keep locus consistent)
  const sidecar = getVersionSidecarPath(binaryPath);
  if (existsSync(sidecar)) {
    try {
      return readFileSync(sidecar, "utf8").trim() !== VERSION;
    } catch {
      return true;
    }
  }
  try {
    const out = execFileSync(binaryPath, ["--version"], {
      stdio: ["ignore", "pipe", "ignore"],
    });
    return !out.toString().includes(VERSION);
  } catch {
    return true;
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
  } finally {
    rmSync(stagingDir, { recursive: true, force: true });
  }
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

function tryExistingOnPath() {
  const which = process.platform === "win32" ? "where" : "which";
  const result = spawnSync(which, [BINARY_NAME], { encoding: "utf8" });
  if (result.status === 0) {
    const path = (result.stdout || "").split(/\r?\n/).map((s) => s.trim()).find(Boolean);
    if (path && existsSync(path)) {
      return path;
    }
  }
  const cargoPath = cargoBinPath();
  if (existsSync(cargoPath)) {
    return cargoPath;
  }
  return null;
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
      "--package",
      CARGO_PACKAGE,
      "--locked",
      "--force",
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

  if (existsSync(binaryPath) && !isCachedBinaryStale(binaryPath)) {
    return binaryPath;
  }
  if (existsSync(binaryPath) && isCachedBinaryStale(binaryPath)) {
    console.error(`Cached locus is out of date — refreshing to v${VERSION}...`);
    removeIfExists(binaryPath);
    removeIfExists(getVersionSidecarPath(binaryPath));
  }

  // Prefer already-installed binary (brew / prior cargo install)
  const existing = tryExistingOnPath();
  if (existing) {
    // If PATH binary is current enough, use it; otherwise still try download
    try {
      if (!isCachedBinaryStale(existing)) {
        return existing;
      }
    } catch {
      // use it anyway if we can't tell
      return existing;
    }
  }

  try {
    return await downloadReleaseBinary(binaryPath);
  } catch (err) {
    console.error(`Failed to download locus: ${err.message}`);
    try {
      return installViaCargo();
    } catch (cargoErr) {
      console.error(cargoErr.message);
      process.exit(1);
    }
  }
}

async function main() {
  const binary = await ensureBinary();
  const args = process.argv.slice(2);

  try {
    execFileSync(binary, args, { stdio: "inherit" });
  } catch (err) {
    process.exit(err.status || 1);
  }
}

if (require.main === module) {
  main();
}

module.exports = {
  SUPPORTED_TARGETS,
  getPlatformTarget,
  parseSha256File,
  findBinaryInDir,
};
