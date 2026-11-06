#!/usr/bin/env node

// Postinstall. On POSIX with a prebuilt release asset, fetch the native binary
// now and link it in place of the JS wrapper, so the user's shell is the
// binary's direct parent (session authority is anchored to that parent; see
// bin/locus.js). Anything else stays lazy: the wrapper downloads (or
// cargo-installs) on first run. Checksum failures fail the npm install rather
// than leaving the integrity problem hidden until first use.

const { existsSync } = require("fs");
const { join } = require("path");

async function run() {
  const wrapper = require("./bin/locus.js");
  const wrapperPath = join(__dirname, "bin", "locus.js");
  if (process.platform === "win32" || process.env.LOCUS_NPM_NO_LINK) {
    console.log("locus will download (or cargo-install) on first use.");
    return;
  }
  let target = null;
  try {
    target = wrapper.getPlatformTarget();
  } catch {}
  const cached = wrapper.getBinaryPath();
  const options = { force: process.argv[2] === "--force" };
  wrapper.assertCanReplace(cached, options);
  let binary = null;
  if (existsSync(cached) && !wrapper.isCachedBinaryStale(cached)) {
    binary = cached;
  } else if (target && wrapper.hasPrebuiltBinary(target)) {
    binary = await wrapper.downloadReleaseBinary(cached, options);
  }
  if (binary && wrapper.linkNativeBin(binary, wrapperPath)) {
    console.log(`locus: linked native binary ${binary}`);
  } else {
    console.log("locus will download (or cargo-install) on first use.");
  }
}

run()
  .catch((err) => {
    if (err.code === "LOCUS_NPM_DOWNGRADE") {
      console.log(`locus: ${err.message}`);
      return;
    }
    if (err.code === "LOCUS_NPM_CHECKSUM") {
      console.error(`locus: ${err.message}`);
      process.exitCode = 1;
      return;
    }
    console.log(`locus will download on first use (${err.message}).`);
  })
  // Never let a lingering socket stall `npm install`.
  .finally(() => process.exit(process.exitCode || 0));
