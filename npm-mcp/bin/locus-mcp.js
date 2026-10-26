#!/usr/bin/env node

/**
 * Thin npm wrapper (published as `@ashlrai/locus-mcp`) for the `locus-mcp` binary.
 *
 * 1. Prefer a cached download under ~/.locus/bin
 * 2. Else download the matching GitHub release asset (locus-<target>.tar.gz)
 * 3. Else fall back to `cargo install --git` and use ~/.cargo/bin/locus-mcp
 *
 * The wrapper owns the child lifetime for stdio clients; HTTP is daemon mode.
 */

const { execFileSync, spawn, spawnSync } = require("child_process");
const {
  chmodSync,
  closeSync,
  openSync,
  readSync,
  realpathSync,
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

const VERSION = "0.5.1";
const REPO = "ashlrai/locus";
const BINARY_NAME = "locus-mcp";
const CARGO_PACKAGE = "locus-mcp";
// Inherited by every launch, including shell shims that hide their target.
const RECURSION_MARKER = "LOCUS_MCP_WRAPPER_ACTIVE";
const INITIAL_PARENT = process.ppid;
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
  // locus-mcp has no --version CLI — use sidecar only
  const sidecar = getVersionSidecarPath(binaryPath);
  if (!existsSync(sidecar)) {
    // Unknown version: treat as usable if file exists (avoid re-download loop)
    return false;
  }
  try {
    return readFileSync(sidecar, "utf8").trim() !== VERSION;
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
    // best-effort
  }
}

function cargoBinPath() {
  const home = process.env.HOME || process.env.USERPROFILE || "/tmp";
  return join(home, ".cargo", "bin", getBinaryFilename());
}

function isNodeShim(path) {
  let fd;
  try {
    const real = realpathSync(path);
    if (real === realpathSync(__filename)) return true;
    // Read a bounded prefix: native binaries can be tens of megabytes.
    fd = openSync(real, "r");
    const prefix = Buffer.alloc(256);
    const head = prefix.subarray(0, readSync(fd, prefix, 0, prefix.length, 0)).toString("utf8");
    return head.startsWith("#!") && /\bnode(?:js)?\b/.test(head.split(/\r?\n/, 1)[0]);
  } catch {
    // An unreadable candidate cannot be established as a native executable.
    return true;
  } finally {
    if (fd !== undefined) closeSync(fd);
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
  if (existsSync(cargoPath) && !isNodeShim(cargoPath)) {
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
  if (!existsSync(path) || isNodeShim(path)) {
    throw new Error("cargo install did not produce a native locus-mcp executable");
  }
  return path;
}

async function downloadReleaseBinary(binaryPath) {
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
    console.error(`Installed locus-mcp to ${binaryPath}`);
    return binaryPath;
  } catch (err) {
    removeIfExists(archivePath);
    throw err;
  }
}

async function ensureBinary() {
  const binaryPath = getBinaryPath();

  if (existsSync(binaryPath) && !isNodeShim(binaryPath) && !isCachedBinaryStale(binaryPath)) {
    return binaryPath;
  }
  if (existsSync(binaryPath) && isCachedBinaryStale(binaryPath)) {
    console.error(`Cached locus-mcp is out of date — refreshing to v${VERSION}...`);
    removeIfExists(binaryPath);
    removeIfExists(getVersionSidecarPath(binaryPath));
  }

  const existing = tryExistingOnPath();
  if (existing) {
    return existing;
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
      return installViaCargo();
    } catch (cargoErr) {
      console.error(cargoErr.message);
      process.exit(1);
    }
  }

  try {
    return await downloadReleaseBinary(binaryPath);
  } catch (err) {
    console.error(`Failed to download locus-mcp: ${err.message}`);
    console.error(cargoFallbackMessage(process, "download-failed"));
    try {
      return installViaCargo();
    } catch (cargoErr) {
      console.error(cargoErr.message);
      process.exit(1);
    }
  }
}

function isHttpMode(args, env = process.env) {
  return args.some((arg) => arg === "--http" || arg === "-H" || arg.startsWith("--http=")) || /^(1|true|on|yes)$/i.test((env.LOCUS_MCP_HTTP || "").trim());
}

function launch(binary, args, httpMode) {
  return new Promise((resolve, reject) => {
    const initialParent = INITIAL_PARENT;
    const grouped = process.platform !== "win32";
    const child = spawn(binary, args, {
      env: { ...process.env, [RECURSION_MARKER]: "1" },
      // A separate group lets cancellation collect shell-shim descendants too.
      detached: grouped,
      stdio: [httpMode ? "inherit" : "pipe", "inherit", "inherit"],
    });
    let stopping = false;
    let timer;
    let watchdog;
    let requestedStatus;
    const signals = { SIGINT: 130, SIGTERM: 143, SIGHUP: 129 };
    const handlers = new Map();
    const kill = (signal) => {
      try {
        if (grouped && child.pid) process.kill(-child.pid, signal);
        else child.kill(signal);
      } catch (err) {
        if (err.code !== "ESRCH") throw err;
      }
    };
    const stop = (status = 0, signal = "SIGTERM", graceful = false) => {
      if (stopping) return;
      stopping = true;
      requestedStatus = graceful ? undefined : status;
      process.stdin.unpipe(child.stdin);
      if (child.stdin) child.stdin.end();
      const terminate = () => {
        requestedStatus = status;
        kill(signal);
        // A server ignoring termination must not keep an ended session alive.
        timer = setTimeout(() => kill("SIGKILL"), 1000);
      };
      // Let the server drain its final request and answer after EOF; bound the
      // grace so a server ignoring EOF still cannot outlive an ended session.
      if (graceful) timer = setTimeout(terminate, 1000);
      else terminate();
    };
    const eof = () => stop(0, "SIGTERM", true);
    const inputError = () => stop(1);
    const cleanup = () => {
      clearInterval(watchdog);
      clearTimeout(timer);
      process.stdin.removeListener("end", eof);
      process.stdin.removeListener("close", eof);
      process.stdin.removeListener("error", inputError);
      if (!httpMode) {
        process.stdin.unpipe(child.stdin);
        process.stdin.pause();
      }
      for (const [signal, handler] of handlers) process.removeListener(signal, handler);
    };
    for (const [signal, status] of Object.entries(signals)) {
      const handler = () => stop(status, signal);
      handlers.set(signal, handler);
      process.on(signal, handler);
    }
    child.once("error", (err) => { cleanup(); reject(err); });
    child.once("close", (code, signal) => {
      // The immediate child may have exited while descendants retained pipes.
      kill("SIGKILL");
      cleanup();
      resolve(requestedStatus ?? code ?? signals[signal] ?? 1);
    });
    if (!httpMode) {
      // A child that fails early can close its input before its exit event.
      // Preserve that child exit code instead of converting EPIPE to success.
      child.stdin.on("error", (err) => { if (err.code !== "EPIPE") stop(1); });
      process.stdin.once("end", eof);
      process.stdin.once("close", eof);
      process.stdin.once("error", inputError);
      process.stdin.pipe(child.stdin);
      if (process.stdin.readableEnded) eof();
      // stdin can remain open in another process after the original client dies.
      watchdog = setInterval(() => {
        if (initialParent <= 1 || process.ppid !== initialParent) stop();
        else {
          try { process.kill(initialParent, 0); }
          catch (err) { if (err.code === "ESRCH") stop(); }
        }
      }, 100);
    }
  });
}

async function main() {
  // Check before resolution/probing: even a copied or shell-disguised shim
  // must never reach a second download, cargo build, or child launch.
  if ([RECURSION_MARKER, "LOCUS_MCP_UPSTREAM_CHILD"].some((key) => Object.prototype.hasOwnProperty.call(process.env, key))) {
    console.error("locus-mcp: refusing recursive npm wrapper launch");
    process.exit(1);
  }
  try {
    const binary = await ensureBinary();
    if (isNodeShim(binary)) throw new Error("refusing to launch a locus-mcp Node shim as a native binary");
    const args = process.argv.slice(2);
    // One-shot CLI queries must print their answer even when stdin is /dev/null.
    const standalone = isHttpMode(args) || args.some((arg) => ["--version", "-V", "--help", "-h"].includes(arg));
    process.exit(await launch(binary, args, standalone));
  } catch (err) {
    console.error(`locus-mcp: ${err.message}`);
    process.exit(1);
  }
}

if (require.main === module) {
  main();
}

module.exports = {
  INSTALL_FROM_SOURCE,
  PREBUILT_TARGETS,
  SUPPORTED_TARGETS,
  cargoFallbackMessage,
  hasPrebuiltBinary,
  getPlatformTarget,
  parseSha256File,
  findBinaryInDir,
};
