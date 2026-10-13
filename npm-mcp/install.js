#!/usr/bin/env node

// Keep installation fast and side-effect-light. The native binary is downloaded
// and verified against package-owned release digests on first run via bin/locus-mcp.js.

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
  console.log("locus-mcp binary cache found; first use will verify its integrity receipt.");
  process.exit(0);
}

console.log("locus-mcp will download and verify a supported release on first use; source installation is manual.");
