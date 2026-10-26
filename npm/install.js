#!/usr/bin/env node

// Postinstall. On POSIX with a prebuilt release asset, fetch the native binary
// now and link it in place of the JS wrapper, so the user's shell is the
// binary's direct parent (session authority is anchored to that parent; see
// bin/locus.js). Anything else stays lazy: the wrapper downloads (or
// cargo-installs) on first run. Never fails the npm install.

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
  let binary = wrapper.findUsableBinary();
  if (!binary && target && wrapper.hasPrebuiltBinary(target)) {
    binary = await wrapper.downloadReleaseBinary(cached);
  }
  if (binary && wrapper.linkNativeBin(binary, wrapperPath)) {
    console.log(`locus: linked native binary ${binary}`);
  } else {
    console.log("locus will download (or cargo-install) on first use.");
  }
}

run()
  .catch((err) => {
    console.log(`locus will download on first use (${err.message}).`);
  })
  // Never let a lingering socket stall `npm install`.
  .finally(() => process.exit(0));
