#!/usr/bin/env node

/**
 * Thin npm wrapper (published as `@ashlrai/locus-mcp`) for the `locus-mcp` binary.
 *
 * 1. Prefer a cached download under ~/.locus/bin
 * 2. Else download the matching GitHub release asset (locus-<target>.tar.gz)
 * 3. Stop on failure; source installation is an explicit, version-pinned choice
 *
 * Note: locus-mcp is a long-running stdio MCP server — do not call it with
 * --version for health checks (it has no CLI version flag).
 */

const { execFileSync } = require("child_process");
const {
  accessSync,
  constants,
  closeSync,
  openSync,
  readSync,
  realpathSync,
  renameSync,
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} = require("fs");
const { delimiter, extname, join, resolve } = require("path");
const https = require("https");
const crypto = require("crypto");

const VERSION = "0.5.0";
const REPO = "ashlrai/locus";
const BINARY_NAME = "locus-mcp";
const CARGO_PACKAGE = "locus-mcp";
// `cargo install` takes the crate as a positional argument; it has no --package flag.
const INSTALL_FROM_SOURCE = `cargo install --git https://github.com/${REPO} --tag v${VERSION} ${CARGO_PACKAGE} --locked`;
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

// Verified against the official live Homebrew tap and all three published
// v0.5.0 release archives. These are asset hashes, not the source tarball hash.
// Update together with VERSION and the release matrix; never trust a checksum
// downloaded beside an archive as a replacement for these package-owned pins.
const RELEASE_SHA256 = Object.freeze({
  "aarch64-apple-darwin": "fd45ec7431a730f309a4532f91356f64d6387143a7ac482fde4440d6ea5b4a79",
  "x86_64-apple-darwin": "5fcb2cf1b229dbcd57bee331df960fade17af5da528fec89913316502f2a9b6a",
  "x86_64-unknown-linux-gnu": "1ce1994a49ad8edd8c30b0e56173c7e45edf2e123002dddc279390a720e361f0",
});
const PREBUILT_TARGETS = Object.freeze(Object.keys(RELEASE_SHA256));

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
    `${why}. No automatic source install was attempted. ` +
    `To install this release explicitly with Rust (https://rustup.rs), run:\n  ${INSTALL_FROM_SOURCE}`
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

function download(url, redirects = 0) {
  return new Promise((resolveDownload, reject) => {
    if (redirects > 5 || !url.startsWith("https://")) {
      reject(new Error("Invalid release download redirect"));
      return;
    }
    const request = https.get(url, (res) => {
      if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
        res.resume();
        try {
          return download(new URL(res.headers.location, url).href, redirects + 1)
            .then(resolveDownload, reject);
        } catch {
          reject(new Error("Invalid release download redirect"));
          return;
        }
      }
      if (res.statusCode !== 200) {
        res.resume();
        reject(new Error(`HTTP ${res.statusCode} for release download`));
        return;
      }
      const chunks = [];
      let size = 0;
      res.on("data", (chunk) => {
        size += chunk.length;
        if (size > 100 * 1024 * 1024) {
          request.destroy(new Error("Release archive exceeds 100 MiB"));
          return;
        }
        chunks.push(chunk);
      });
      res.on("end", () => resolveDownload(Buffer.concat(chunks)));
      res.on("error", reject);
    });
    request.setTimeout(30_000, () => request.destroy(new Error("Release download timed out")));
    request.on("error", reject);
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

function sha256(data) {
  return crypto.createHash("sha256").update(data).digest("hex");
}

function isCachedBinaryStale(binaryPath) {
  // Old version-only sidecars cannot prove a verified install. Do not execute
  // an unknown cached binary (especially an MCP server) to inspect its version.
  try {
    const receipt = JSON.parse(readFileSync(`${binaryPath}.integrity.json`, "utf8"));
    const target = getPlatformTarget();
    return receipt.version !== VERSION || receipt.target !== target ||
      !RELEASE_SHA256[target] || receipt.archiveSha256 !== RELEASE_SHA256[target] ||
      receipt.binarySha256 !== sha256(readFileSync(binaryPath));
  } catch {
    return true;
  }
}

function writeCacheReceipt(binaryPath, target, stagingDir) {
  const receipt = {
    version: VERSION,
    target,
    archiveSha256: RELEASE_SHA256[target],
    binarySha256: sha256(readFileSync(binaryPath)),
  };
  const stagedReceipt = join(stagingDir, "integrity.json");
  writeFileSync(stagedReceipt, `${JSON.stringify(receipt)}\n`, "utf8");
  renameSync(stagedReceipt, `${binaryPath}.integrity.json`);
  writeFileSync(getVersionSidecarPath(binaryPath), `${VERSION}\n`, "utf8");
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

function extractBinaryFromArchive(archivePath, binaryPath, target) {
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

    const stagedBinary = join(stagingDir, ".installed-binary");
    copyFileSync(extractedBinaryPath, stagedBinary);
    if (process.platform !== "win32") chmodSync(stagedBinary, 0o755);
    // Atomic replacement keeps an older cache intact until extraction succeeds.
    renameSync(stagedBinary, binaryPath);
    writeCacheReceipt(binaryPath, target, stagingDir);
  } finally {
    rmSync(stagingDir, { recursive: true, force: true });
  }
}

function cargoBinPath() {
  const home = process.env.HOME || process.env.USERPROFILE || "/tmp";
  return join(home, ".cargo", "bin", getBinaryFilename());
}

function isNativeCandidate(candidate) {
  let fd;
  try {
    const actual = realpathSync(candidate);
    if (actual === realpathSync(__filename) || resolve(candidate) === resolve(getBinaryPath())) return false;
    if (!statSync(actual).isFile()) return false;
    // Windows npm .cmd/.ps1 launchers and JavaScript copies are never native.
    if ([".js", ".cmd", ".ps1"].includes(extname(actual).toLowerCase())) return false;
    if (process.platform !== "win32") accessSync(actual, constants.X_OK);
    fd = openSync(actual, "r");
    const header = Buffer.alloc(4096);
    const size = readSync(fd, header, 0, header.length, 0);
    const text = header.subarray(0, size).toString("utf8");
    // npm's Unix launcher normally resolves to this file, but another global
    // package copy or a shell launcher can also invoke Node recursively.
    if (text.startsWith("#!") && /\bnode(?:\.exe)?\b/.test(text)) return false;
    return true;
  } catch {
    return false;
  } finally {
    if (fd !== undefined) closeSync(fd);
  }
}

function tryExistingOnPath() {
  // Search every entry rather than accepting the first `which` result (often
  // our own global npm shim). Paths are passed directly, including spaces.
  const filename = getBinaryFilename();
  const candidates = (process.env.PATH || "").split(delimiter)
    .filter(Boolean).map((dir) => join(dir, filename));
  candidates.push(cargoBinPath());
  for (const candidate of candidates) {
    if (!isNativeCandidate(candidate)) continue;
    return candidate;
  }
  return null;
}

async function downloadReleaseBinary(binaryPath) {
  const target = getPlatformTarget();
  const expected = RELEASE_SHA256[target];
  if (!expected) throw new Error(cargoFallbackMessage(process, "no-prebuilt"));
  const archiveName = `locus-${target}.tar.gz`;
  const url = `https://github.com/${REPO}/releases/download/v${VERSION}/${archiveName}`;
  console.error(`Downloading locus-mcp v${VERSION} for ${target}...`);
  const data = await download(url);
  if (sha256(data) !== expected) {
    throw new Error(`SHA-256 mismatch for ${archiveName}; installation refused`);
  }
  mkdirSync(CACHE_DIR, { recursive: true });
  // Each invocation owns its archive, so CLI and MCP installs can run together.
  const downloadDir = mkdtempSync(join(CACHE_DIR, ".download-"));
  try {
    const archivePath = join(downloadDir, archiveName);
    writeFileSync(archivePath, data);
    extractBinaryFromArchive(archivePath, binaryPath, target);
    console.error(`Installed locus-mcp to ${binaryPath}`);
    return binaryPath;
  } finally {
    rmSync(downloadDir, { recursive: true, force: true });
  }
}

async function ensureBinary() {
  const binaryPath = getBinaryPath();
  if (existsSync(binaryPath) && !isCachedBinaryStale(binaryPath)) return binaryPath;
  const existing = tryExistingOnPath();
  if (existing) return existing;
  // Fail closed: a download, integrity, or platform failure never installs
  // arbitrary repository HEAD through an automatic Cargo fallback.
  try {
    return await downloadReleaseBinary(binaryPath);
  } catch (err) {
    throw new Error(`${err.message}\nManual source install: ${INSTALL_FROM_SOURCE}`);
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
  main().catch((err) => {
    console.error(err.message);
    process.exit(1);
  });
}

module.exports = {
  INSTALL_FROM_SOURCE,
  RELEASE_SHA256,
  PREBUILT_TARGETS,
  SUPPORTED_TARGETS,
  cargoFallbackMessage,
  hasPrebuiltBinary,
  getPlatformTarget,
  parseSha256File,
  findBinaryInDir,
  tryExistingOnPath,
  ensureBinary,
  isCachedBinaryStale,
};
