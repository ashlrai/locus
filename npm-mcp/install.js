#!/usr/bin/env node

// Keep installation fast and side-effect-light. The native binary downloads
// (or is cargo-installed) lazily on first run via bin/locus-mcp.js.

const { existsSync } = require("fs");
const wrapper = require("./bin/locus-mcp.js");

const binaryPath = wrapper.getBinaryPath();

if (process.platform === "win32") {
  try {
    const target = wrapper.getPlatformTarget();
    if (!wrapper.hasPrebuiltBinary(target)) {
      console.error(wrapper.missingWindowsBinaryMessage(target));
      process.exit(1);
    }
  } catch (err) {
    console.error(err.message);
    process.exit(1);
  }
}

if (existsSync(binaryPath)) {
  console.log("locus-mcp binary already installed.");
  process.exit(0);
}

console.log("locus-mcp will download (or cargo-install) on first use.");
