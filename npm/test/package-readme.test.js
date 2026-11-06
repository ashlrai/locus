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

// Join shell continuation lines, but keep separate commands separate. This
// detects known same-command and default-root sequential collision patterns;
// it is a documentation regression guard, not a general shell parser.
function hasCollidingCargoInstall(text) {
  const source = text.replace(/<[^>]*>/g, "").replace(/\\\r?\n\s*/g, " ");
  const commands = source.split(/\r?\n|&&/).map((line) => line.trim()).filter(Boolean);
  const dual = commands.some((line) => /\bcargo\s+(?:\+\S+\s+)?install\b/.test(line)
    && /(?:^|\s)locus-cli(?=\s|$)/.test(line)
    && /(?:^|\s)locus-mcp(?=\s|$)/.test(line));
  // Compare adjacent install commands only when their roots are both default
  // or the same literal. This is deliberately not a shell parser.
  const invocation = (line) => {
    if (!/\bcargo\s+(?:\+\S+\s+)?install\b/.test(line)) return null;
    const packages = ["locus-cli", "locus-mcp"].filter((name) =>
      new RegExp(`(?:^|\\s)${name}(?=\\s|$)`).test(line)
      || line.includes(`--path crates/${name}`));
    const root = /(?:^|\s)--root(?:=|\s+)(?:"([^"]+)"|'([^']+)'|(\S+))/.exec(line);
    return { packages, root: root ? (root[1] ?? root[2] ?? root[3]) : null };
  };
  const sequential = commands.some((line, index) => {
    const first = invocation(line);
    const second = invocation(commands[index + 1] || "");
    return first && second && first.root === second.root
      && first.packages.length === 1 && second.packages.length === 1
      && first.packages[0] !== second.packages[0];
  });
  return dual || sequential;
}

test("cargo documentation collision guard detects the known patterns", () => {
  assert.equal(hasCollidingCargoInstall("cargo install --git url locus-cli locus-mcp --locked"), true);
  assert.equal(hasCollidingCargoInstall("cargo install --git url locus-mcp \\\n  --locked locus-cli"), true);
  assert.equal(hasCollidingCargoInstall("cargo install --path crates/locus-cli\ncargo install --path crates/locus-mcp"), true);
  assert.equal(hasCollidingCargoInstall("cargo install --path crates/locus-cli && cargo install --path crates/locus-mcp"), true);
  assert.equal(hasCollidingCargoInstall("cargo +1.89.0 install --path crates/locus-cli\ncargo +1.89.0 install --path crates/locus-mcp"), true);
  assert.equal(hasCollidingCargoInstall("cargo install --path crates/locus-mcp && cargo install --path crates/locus-cli"), true);
  assert.equal(hasCollidingCargoInstall("cargo install --git url locus-cli\ncargo install --git url locus-mcp"), true);
  assert.equal(hasCollidingCargoInstall("cargo install --git url locus-mcp --root /tmp/a && cargo install --git url locus-cli --root /tmp/a"), true);
  assert.equal(hasCollidingCargoInstall("cargo install --path crates/locus-cli --root /tmp/a\ncargo install --path crates/locus-mcp --root /tmp/b"), false);
  assert.equal(hasCollidingCargoInstall("cargo install --git url locus-cli --root /tmp/a && cargo install --git url locus-mcp --root /tmp/b"), false);
  assert.equal(hasCollidingCargoInstall("cargo install --git url locus-cli --locked"), false);
  assert.equal(hasCollidingCargoInstall("cargo install --git url locus-mcp --locked"), false);
});

test("current source-install guidance avoids the colliding combined or sequential commands", () => {
  // Scope the guidance to the checked-out source, not the separately published tag.
  const manifest = readFileSync(join(root, "crates/locus-cli/Cargo.toml"), "utf8");
  const binNames = manifest.split(/^\[\[bin\]\][ \t]*$/m).slice(1)
    .map((section) => section.split(/^\[/m, 1)[0].match(/^name\s*=\s*"([^"]+)"/m)?.[1]);
  assert.ok(binNames.includes("locus") && binNames.includes("locus-mcp"), "source package must install both documented binaries");
  for (const path of ["AGENTS.md", "CONTRIBUTING.md", "docs/mcp.md", "docs/onboarding.md",
    "docs/dogfood-multi-account.md", "integrations/homebrew/README.md", "apps/web/public/index.html",
    "apps/web/public/AGENTS.md"]) {
    assert.equal(hasCollidingCargoInstall(readFileSync(join(root, path), "utf8")), false, path);
  }
  const capabilities = JSON.parse(readFileSync(join(root, "apps/web/public/capabilities.json"), "utf8"));
  assert.equal(capabilities.install.local, "cargo install --locked --path crates/locus-cli");
  assert.equal(hasCollidingCargoInstall(capabilities.install.local), false);
});

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
        assert.equal(hasCollidingCargoInstall(readme), false, `colliding Cargo instructions in packed ${dir} README`);
        if (candidate === "9.8.7") assert.ok(!readme.includes(candidate), "future fixture must not be advertised as published");
      }
    } finally {
      rmSync(temp, { recursive: true, force: true });
    }
  });
}
