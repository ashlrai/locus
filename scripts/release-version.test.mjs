import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { carriers, check, cargoVersion, nextVersion, sync } from "./release-version.mjs";

const repo = fileURLToPath(new URL("..", import.meta.url));
const script = fileURLToPath(new URL("./release-version.mjs", import.meta.url));
const fixtureFiles = [
  "Cargo.toml", "Cargo.lock", "server.json", "npm/package.json", "npm-mcp/package.json",
  "apps/web/package.json", "npm/bin/locus.js", "npm-mcp/bin/locus-mcp.js",
  "crates/locus-cli/Cargo.toml", "scripts/freshness/published.json",
];

// A copy of the real carriers, so the tests track the repo's actual formats.
function fixture(t) {
  const root = mkdtempSync(join(tmpdir(), "release-version-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  for (const rel of fixtureFiles) {
    mkdirSync(join(root, rel, ".."), { recursive: true });
    cpSync(join(repo, rel), join(root, rel));
  }
  return root;
}

test("the repository's carriers are consistent", () => {
  assert.deepEqual(check(repo), []);
});

test("every source carrier kind is covered including path dependencies and wrapper constants", () => {
  const labels = carriers(repo).map(([l]) => l);
  for (const want of ["Cargo.toml [workspace.package]", "Cargo.lock locus-cli", "Cargo.lock locus-core", "Cargo.lock locus-mcp", "npm/package.json", "npm-mcp/package.json", "apps/web/package.json", "npm/bin/locus.js VERSION", "npm-mcp/bin/locus-mcp.js VERSION", "Cargo.toml path dependency locus-core", "crates/locus-cli/Cargo.toml path dependency locus-mcp", "server.json version", "server.json packages[0] @ashlrai/locus-mcp"]) {
    assert.ok(labels.includes(want), `missing carrier: ${want}`);
  }
  assert.ok(!labels.some((l) => l.startsWith("Cargo.lock") && !/locus-(cli|core|mcp)$/.test(l)), "registry crates must not be treated as carriers");
});

test("sync moves every carrier and leaves registry crates and dependency ranges alone", (t) => {
  const root = fixture(t);
  const lockBefore = readFileSync(join(root, "Cargo.lock"), "utf8");
  const publishedBefore = readFileSync(join(root, "scripts/freshness/published.json"), "utf8");
  assert.deepEqual(sync(root, "9.8.7"), []);
  assert.equal(cargoVersion(root), "9.8.7");
  for (const [label, v] of carriers(root)) assert.equal(v, "9.8.7", label);
  const lockAfter = readFileSync(join(root, "Cargo.lock"), "utf8");
  const changed = lockAfter.split("\n").filter((l, i) => l !== lockBefore.split("\n")[i]);
  assert.equal(changed.length, 3, "only the three workspace crate versions change");
  assert.equal(readFileSync(join(root, "scripts/freshness/published.json"), "utf8"), publishedBefore,
    "a source version change must not claim publication");
  assert.deepEqual(check(root), []);
  const synced = fixtureFiles.map((rel) => readFileSync(join(root, rel), "utf8"));
  assert.deepEqual(sync(root, "9.8.7"), []);
  assert.deepEqual(fixtureFiles.map((rel) => readFileSync(join(root, rel), "utf8")), synced,
    "sync is byte-for-byte idempotent");
});

test("check detects drift in web, wrapper and local dependency versions", (t) => {
  const root = fixture(t);
  for (const [rel, from, to, label] of [
    ["apps/web/package.json", /("version": ")[^"]+/, '$10.0.1', "apps/web/package.json"],
    ["npm/bin/locus.js", /const VERSION = "[^"]+/, 'const VERSION = "0.0.1', "npm/bin/locus.js VERSION"],
    ["npm-mcp/bin/locus-mcp.js", /const VERSION = "[^"]+/, 'const VERSION = "0.0.1', "npm-mcp/bin/locus-mcp.js VERSION"],
    ["Cargo.toml", /(locus-core = \{[^\n]*version = ")[^"]+/, '$10.0.1', "Cargo.toml path dependency locus-core"],
    ["crates/locus-cli/Cargo.toml", /(locus-mcp = \{[^\n]*version = ")[^"]+/, '$10.0.1', "crates/locus-cli/Cargo.toml path dependency locus-mcp"],
  ]) {
    const file = join(root, rel);
    const before = readFileSync(file, "utf8");
    writeFileSync(file, before.replace(from, to));
    assert.deepEqual(check(root), [`${label} is 0.0.1, Cargo is ${cargoVersion(root)}`]);
    writeFileSync(file, before);
  }
});

test("sync changes local path versions while preserving registry dependency versions", (t) => {
  const root = fixture(t);
  const manifest = join(root, "crates/locus-cli/Cargo.toml");
  const dependency = '\nrelease-test-registry = { version = "4.5.6" }\n';
  writeFileSync(manifest, readFileSync(manifest, "utf8") + dependency);
  assert.deepEqual(sync(root, "9.8.7"), []);
  assert.ok(readFileSync(manifest, "utf8").endsWith(dependency));
});

test("check names every drifted carrier and a wrong tag", (t) => {
  const root = fixture(t);
  const pkg = join(root, "npm/package.json");
  writeFileSync(pkg, readFileSync(pkg, "utf8").replace(/"version": "[^"]+"/, '"version": "0.0.1"'));
  const server = join(root, "server.json");
  writeFileSync(server, readFileSync(server, "utf8").replace(/^( {6}"version": ")[^"]+/m, "$10.0.2"));
  const want = cargoVersion(root);
  assert.deepEqual(check(root, `v${want}`), [
    `npm/package.json is 0.0.1, Cargo is ${want}`,
    `server.json packages[0] @ashlrai/locus-mcp is 0.0.2, Cargo is ${want}`,
  ]);
  assert.deepEqual(sync(root, want), [], "sync repairs drift");
  assert.deepEqual(check(root, "v0.0.0"), [`tag v0.0.0 is not v${want}`]);
});

test("next: patch for bridge/docs/fix, minor for feat, breaking per major", () => {
  assert.equal(nextVersion("0.6.2", ["bridge(RQ-X): notes (#1)", "docs: x"]), "0.6.3");
  assert.equal(nextVersion("0.6.2", ["fix: y", "feat(cli): z"]), "0.7.0");
  assert.equal(nextVersion("0.6.2", ["refactor!: drop flag"]), "0.7.0");
  assert.equal(nextVersion("1.4.9", ["feat!: new contract"]), "2.0.0");
  assert.equal(nextVersion("1.4.9", ["chore: BREAKING CHANGE in body"]), "2.0.0");
  assert.equal(nextVersion("1.4.9", ["feat: a"]), "1.5.0");
  assert.equal(nextVersion("1.4.9", ["", "  "]), null, "no commits, no release");
  assert.throws(() => nextVersion("1.4", ["fix: a"]));
});

test("sync rejects a non-semver version before touching files", (t) => {
  const root = fixture(t);
  assert.throws(() => sync(root, "1.2"));
  assert.throws(() => sync(root, "1.2.3-rc.1"));
  assert.deepEqual(check(root), []);
});

test("CLI: check passes on the repo, fails closed on a wrong tag, next reads stdin", () => {
  const env = { ...process.env, RELEASE_VERSION_ROOT: repo };
  const v = cargoVersion(repo);
  assert.equal(spawnSync("node", [script, "check", "--tag", `v${v}`], { env }).status, 0);
  const bad = spawnSync("node", [script, "check", "--tag", "v0.0.0"], { env, encoding: "utf8" });
  assert.equal(bad.status, 1);
  assert.match(bad.stderr, /tag v0\.0\.0 is not v/);
  const next = spawnSync("node", [script, "next"], { env, input: "bridge(RQ-A): x\n", encoding: "utf8" });
  assert.equal(next.stdout.trim(), nextVersion(v, ["bridge(RQ-A): x"]));
  assert.equal(spawnSync("node", [script, "nope"], { env }).status, 1);
  assert.equal(spawnSync("node", [script, "check", "--tag"], { env }).status, 1);
});
