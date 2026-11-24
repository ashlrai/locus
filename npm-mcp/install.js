#!/usr/bin/env node

// Keep installation fast and side-effect-light. The native binary downloads
// (or is cargo-installed) lazily on first run via bin/locus-mcp.js.

const { existsSync } = require("fs");
const os = require("os");
const { join } = require("path");

function userHome() {
  const explicit = [process.env.HOME, process.env.USERPROFILE].find(
    (value) => typeof value === "string" && value.trim() !== ""
  );
  if (explicit) return explicit;
  try {
    const home = os.homedir();
    if (home) return home;
  } catch {
    // os.homedir throws when the account has no home directory.
  }
  return os.tmpdir();
}

const CACHE_DIR = join(userHome(), ".locus", "bin");

const binaryExt = process.platform === "win32" ? ".exe" : "";
const binaryPath = join(CACHE_DIR, `locus-mcp${binaryExt}`);

if (existsSync(binaryPath)) {
  console.log("locus-mcp binary already installed.");
  process.exit(0);
}

console.log("locus-mcp will download (or cargo-install) on first use.");
