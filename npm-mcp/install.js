#!/usr/bin/env node

// Keep installation fast and side-effect-light. The native binary downloads
// (or is cargo-installed) lazily on first run via bin/locus-mcp.js.

const { existsSync } = require("fs");
const { join } = require("path");

const CACHE_DIR = join(
  process.env.HOME || process.env.USERPROFILE || "/tmp",
  ".locus",
  "bin"
);

const binaryExt = process.platform === "win32" ? ".exe" : "";
const binaryPath = join(CACHE_DIR, `locus-mcp${binaryExt}`);

if (existsSync(binaryPath)) {
  console.log("locus-mcp binary already installed.");
  process.exit(0);
}

console.log("locus-mcp will download (or cargo-install) on first use.");
