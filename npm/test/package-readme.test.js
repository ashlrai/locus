// Package READMEs travel in immutable npm tarballs and must not pin stale releases.
const assert = require("node:assert/strict");
const { cpSync, mkdtempSync, readFileSync, rmSync, writeFileSync } = require("node:fs");
const { tmpdir } = require("node:os");
const { join } = require("node:path");
const { spawnSync } = require("node:child_process");
const { gunzipSync } = require("node:zlib");
const test = require("node:test");

const root = join(__dirname, "..", "..");
const npm = process.platform === "win32" ? "npm.cmd" : "npm";

function command(executable, args, options = {}) {
  const result = spawnSync(executable, args, { encoding: "utf8", ...options });
  assert.equal(result.status, 0, `${executable} failed: ${result.stderr || result.error}`);
  return result.stdout;
}

// npm creates a gzip-compressed ustar archive. Read only the two short paths
// needed here; no platform tar executable is required.
function archiveText(archive, target) {
  const data = gunzipSync(readFileSync(archive));
  for (let offset = 0; offset + 512 <= data.length;) {
    const header = data.subarray(offset, offset + 512);
    const name = header.toString("utf8", 0, 100).replace(/\0.*$/, "");
    if (!name) break;
    const sizeText = header.toString("ascii", 124, 136).replace(/\0.*$/, "").trim();
    const size = Number.parseInt(sizeText || "0", 8);
    assert.ok(Number.isSafeInteger(size) && size >= 0 && offset + 512 + size <= data.length);
    if (name === target) return data.toString("utf8", offset + 512, offset + 512 + size);
    offset += 512 + Math.ceil(size / 512) * 512;
  }
  throw new Error(`missing ${target} in npm archive`);
}

for (const dir of ["npm", "npm-mcp"]) {
  test(`${dir} packed README uses version-neutral installs for current and future source`, () => {
    const temp = mkdtempSync(join(tmpdir(), "locus-pack-readme-"));
    try {
      const original = join(root, dir);
      const current = JSON.parse(readFileSync(join(original, "package.json"), "utf8"));
      for (const candidate of [current.version, "9.8.7"]) {
        const fixture = join(temp, `source-${candidate}`);
        cpSync(original, fixture, { recursive: true });
        const pkg = JSON.parse(readFileSync(join(fixture, "package.json"), "utf8"));
        pkg.version = candidate;
        writeFileSync(join(fixture, "package.json"), JSON.stringify(pkg, null, 2) + "\n");
        const env = { ...process.env, HOME: temp, npm_config_cache: join(temp, "cache"), npm_config_offline: "true" };
        // All shell arguments are fixed literals; cwd, not a shell argument,
        // identifies the fixture. Windows needs a shell to run npm.cmd.
        const packed = JSON.parse(command(npm, ["pack", "--json", "--offline", "--ignore-scripts", "--pack-destination", "."], {
          cwd: fixture, env, shell: process.platform === "win32",
        }));
        assert.equal(packed.length, 1);
        const archive = join(fixture, packed[0].filename);
        const archivedManifest = JSON.parse(archiveText(archive, "package/package.json"));
        const readme = archiveText(archive, "package/README.md");
        assert.equal(archivedManifest.name, current.name);
        assert.equal(archivedManifest.version, candidate);
        assert.match(readme, /^npm install -g locus-cli @ashlrai\/locus-mcp$/m);
        const commands = readme.split(/\r?\n/).filter((line) => /^\s*(?:npm\s+(?:install|i)\b|npx\b)/.test(line));
        assert.ok(commands.length > 0);
        for (const line of commands) {
          assert.doesNotMatch(line, /(?:locus-cli|@ashlrai\/locus-mcp)@\S+/, `versioned install in ${dir}: ${line}`);
        }
        if (candidate === "9.8.7") assert.ok(!readme.includes(candidate), "future fixture must not be advertised as published");
      }
    } finally {
      rmSync(temp, { recursive: true, force: true });
    }
  });
}
