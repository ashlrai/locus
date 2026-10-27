// Manifest-only changes must run the offline Rust registry contract in CI.
const assert = require("node:assert/strict");
const { spawnSync } = require("node:child_process");
const { mkdtempSync, mkdirSync, writeFileSync, rmSync } = require("node:fs");
const { tmpdir } = require("node:os");
const { join } = require("node:path");
const test = require("node:test");

const root = join(__dirname, "..", "..");
const selector = join(root, "scripts", "ci", "affected-filterset.sh");

for (const [file, expected] of [
  ["server.json", "all()"],
  ["npm-mcp/package.json", "all()"],
  ["README.md", "none()"],
  ["crates/locus-mcp/src/main.rs", "rdeps(=locus-mcp)"],
]) {
  test(`affected tests for ${file}: ${expected}`, () => {
    const home = mkdtempSync(join(tmpdir(), "locus-registry-filter-"));
    try {
      const bin = join(home, "bin");
      mkdirSync(bin);
      // Stub only Git's changed paths and Cargo's workspace inventory. Exercise
      // the actual selector without making commits or invoking a build.
      const metadata = {
        workspace_root: root,
        packages: ["locus-core", "locus-cli", "locus-mcp"].map((name) => ({
          name,
          manifest_path: join(root, "crates", name, "Cargo.toml"),
        })),
      };
      for (const [name, output] of [
        ["git", file],
        ["cargo", JSON.stringify(metadata)],
      ]) {
        writeFileSync(
          join(bin, name),
          `#!/usr/bin/env node\nprocess.stdout.write(${JSON.stringify(output + "\n")});\n`,
          { mode: 0o755 },
        );
      }
      const result = spawnSync("bash", [selector, "base", "head"], {
        cwd: root,
        env: { HOME: home, PATH: `${bin}:${process.env.PATH}` },
        encoding: "utf8",
        timeout: 20_000,
      });
      assert.equal(result.status, 0, result.stderr);
      assert.equal(result.stdout.trim(), expected);
    } finally {
      rmSync(home, { recursive: true, force: true });
    }
  });
}
