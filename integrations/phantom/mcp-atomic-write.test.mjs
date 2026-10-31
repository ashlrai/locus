import assert from "node:assert/strict";
import fs from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { syncBuiltinESMExports } from "node:module";
import { spawnSync } from "node:child_process";
import test from "node:test";
import * as phantom from "./locus.ts";
import * as legacyHub from "../ashlr-hub/locus.ts";

const original = '{"mcpServers":{"other":{"command":"keep-me"}},"unrelated":true}\n';
const options = { locusHome: "/synthetic/locus-home", client: "atomic-test" };
function fixture(t, contents = original) {
  const dir = fs.mkdtempSync(join(tmpdir(), "locus-atomic-test-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const path = join(dir, "mcp.json");
  if (contents !== null) fs.writeFileSync(path, contents, { mode: 0o640 });
  return { dir, path };
}
function fault(t, method, impl) {
  const real = fs[method];
  t.mock.method(fs, method, impl(real));
  syncBuiltinESMExports();
  t.after(() => { t.mock.restoreAll(); syncBuiltinESMExports(); });
}
for (const [name, locus] of [["phantom", phantom], ["ashlr-hub", legacyHub]]) {
  test(`${name}: partial write failure leaves original bytes and mode intact`, (t) => {
    const { dir, path } = fixture(t);
    const before = fs.statSync(path);
    fault(t, "writeFileSync", (real) => (...args) => {
      real(args[0], String(args[1]).slice(0, 8), args[2]);
      throw new Error("synthetic disk write failure");
    });
    assert.throws(() => locus.registerLocusInMcpConfig(path, options), locus.LocusMcpConfigError);
    assert.equal(fs.readFileSync(path, "utf8"), original);
    assert.equal(fs.statSync(path).mode, before.mode);
    assert.deepEqual(fs.readdirSync(dir), ["mcp.json"]);
  });
  for (const method of ["openSync", "fsyncSync", "renameSync", "fchownSync", "fchmodSync"]) {
    test(`${name}: ${method} failure does not replace original and removes staging file`, { skip: process.platform === "win32" && ["fchownSync", "fchmodSync"].includes(method) }, (t) => {
      const { dir, path } = fixture(t);
      fault(t, method, () => () => { throw new Error(`synthetic ${method} failure`); });
      assert.throws(() => locus.registerLocusInMcpConfig(path, options), locus.LocusMcpConfigError);
      assert.equal(fs.readFileSync(path, "utf8"), original);
      assert.deepEqual(fs.readdirSync(dir), ["mcp.json"]);
    });
  }
  test(`${name}: successful replacement preserves mode, owner and unrelated keys`, (t) => {
    const { dir, path } = fixture(t);
    const before = fs.statSync(path);
    const result = locus.registerLocusInMcpConfig(path, options);
    assert.equal(result.written, true);
    const after = fs.statSync(path);
    assert.equal(after.mode & 0o7777, before.mode & 0o7777);
    assert.equal(after.uid, before.uid);
    assert.equal(after.gid, before.gid);
    assert.equal(result.config.unrelated, true);
    assert.equal(result.config.mcpServers.other.command, "keep-me");
    assert.deepEqual(fs.readdirSync(dir), ["mcp.json"]);
  });
  test(`${name}: creates private nested config and remains idempotent`, (t) => {
    const { dir } = fixture(t, null);
    const path = join(dir, "nested", "mcp.json");
    const result = locus.registerLocusInMcpConfig(path, options);
    assert.equal(result.created, true);
    assert.equal(result.written, true);
    if (process.platform !== "win32") assert.equal(fs.statSync(path).mode & 0o777, 0o600);
    const before = fs.readFileSync(path, "utf8");
    assert.equal(locus.registerLocusInMcpConfig(path, options).written, false);
    assert.equal(fs.readFileSync(path, "utf8"), before);
    assert.deepEqual(fs.readdirSync(join(dir, "nested")), ["mcp.json"]);
  });
  test(`${name}: failed creation leaves no partial destination`, (t) => {
    const { dir, path } = fixture(t, null);
    fault(t, "writeFileSync", (real) => (...args) => {
      real(args[0], "partial", args[2]);
      throw new Error("synthetic write failure");
    });
    assert.throws(() => locus.registerLocusInMcpConfig(path, options), locus.LocusMcpConfigError);
    assert.equal(fs.existsSync(path), false);
    assert.deepEqual(fs.readdirSync(dir), []);
  });
  test(`${name}: close failure leaves original and cleans staging file`, (t) => {
    const { dir, path } = fixture(t);
    let once = true;
    fault(t, "closeSync", (real) => (fd) => {
      real(fd);
      if (once) { once = false; throw new Error("synthetic close failure"); }
    });
    assert.throws(() => locus.registerLocusInMcpConfig(path, options), locus.LocusMcpConfigError);
    assert.equal(fs.readFileSync(path, "utf8"), original);
    assert.deepEqual(fs.readdirSync(dir), ["mcp.json"]);
  });
  test(`${name}: refuses nonregular directory target`, (t) => {
    const { dir, path } = fixture(t);
    assert.throws(() => locus.registerLocusInMcpConfig(dir, options), locus.LocusMcpConfigError);
    assert.equal(fs.readFileSync(path, "utf8"), original);
  });
  test(`${name}: access refusal does not replace original`, (t) => {
    const { dir, path } = fixture(t);
    fault(t, "accessSync", () => () => { throw new Error("synthetic access refusal"); });
    assert.throws(() => locus.registerLocusInMcpConfig(path, options), locus.LocusMcpConfigError);
    assert.equal(fs.readFileSync(path, "utf8"), original);
    assert.deepEqual(fs.readdirSync(dir), ["mcp.json"]);
  });
  test(`${name}: unchanged config does not require write access`, (t) => {
    const { path } = fixture(t);
    locus.registerLocusInMcpConfig(path, options);
    const before = fs.readFileSync(path, "utf8");
    fault(t, "accessSync", () => () => { throw new Error("synthetic access refusal"); });
    assert.equal(locus.registerLocusInMcpConfig(path, options).written, false);
    assert.equal(fs.readFileSync(path, "utf8"), before);
  });
  test(`${name}: termination mid-write cannot truncate the existing config`, { skip: process.platform === "win32" }, (t) => {
    const { dir, path } = fixture(t);
    const moduleUrl = new URL(name === "phantom" ? "./locus.ts" : "../ashlr-hub/locus.ts", import.meta.url).href;
    const result = spawnSync(process.execPath, ["--experimental-strip-types", "--input-type=module", "-e", `
      import fs from "node:fs";
      import { syncBuiltinESMExports } from "node:module";
      const real = fs.writeFileSync;
      fs.writeFileSync = (...args) => {
        real(args[0], "partial", args[2]);
        process.kill(process.pid, "SIGKILL");
      };
      syncBuiltinESMExports();
      const locus = await import(${JSON.stringify(moduleUrl)});
      locus.registerLocusInMcpConfig(${JSON.stringify(path)}, ${JSON.stringify(options)});
    `], { encoding: "utf8", env: { PATH: process.env.PATH }, timeout: 10000 });
    assert.equal(result.error, undefined);
    assert.equal(result.signal, "SIGKILL");
    assert.equal(fs.readFileSync(path, "utf8"), original);
    const orphans = fs.readdirSync(dir).filter((file) => file !== "mcp.json");
    assert.equal(orphans.length, 1, "uncatchable interruption may leave a private staging file");
    assert.equal(fs.statSync(join(dir, orphans[0])).mode & 0o777, 0o600);
  });
  test(`${name}: termination after metadata retains original and leaves staging at the old mode`, { skip: process.platform === "win32" }, (t) => {
    const { dir, path } = fixture(t);
    const before = fs.statSync(path);
    const moduleUrl = new URL(name === "phantom" ? "./locus.ts" : "../ashlr-hub/locus.ts", import.meta.url).href;
    const result = spawnSync(process.execPath, ["--experimental-strip-types", "--input-type=module", "-e", `
      import fs from "node:fs";
      import { syncBuiltinESMExports } from "node:module";
      // The writer applies preserved ownership/mode immediately before fsync.
      fs.fsyncSync = () => process.kill(process.pid, "SIGKILL");
      syncBuiltinESMExports();
      const locus = await import(${JSON.stringify(moduleUrl)});
      locus.registerLocusInMcpConfig(${JSON.stringify(path)}, ${JSON.stringify(options)});
    `], { encoding: "utf8", env: { PATH: process.env.PATH }, timeout: 10000 });
    assert.equal(result.error, undefined);
    assert.equal(result.signal, "SIGKILL");
    assert.equal(fs.readFileSync(path, "utf8"), original);
    const orphans = fs.readdirSync(dir).filter((file) => file !== "mcp.json");
    assert.equal(orphans.length, 1);
    const staged = join(dir, orphans[0]);
    const metadata = fs.statSync(staged);
    assert.equal(metadata.mode & 0o7777, before.mode & 0o7777);
    assert.equal(metadata.uid, before.uid);
    assert.equal(metadata.gid, before.gid);
    assert.equal(JSON.parse(fs.readFileSync(staged, "utf8")).mcpServers.other.command, "keep-me");
  });
  for (const dangling of [false, true]) {
    test(`${name}: refuses ${dangling ? "dangling" : "existing"} final symlink without touching it or its target`, (t) => {
      const { dir, path } = fixture(t);
      const link = join(dir, "link.json");
      const target = dangling ? join(dir, "missing.json") : path;
      fs.symlinkSync(target, link);
      assert.throws(() => locus.registerLocusInMcpConfig(link, options), locus.LocusMcpConfigError);
      assert.ok(fs.lstatSync(link).isSymbolicLink());
      assert.equal(fs.readFileSync(path, "utf8"), original);
      if (dangling) assert.equal(fs.existsSync(target), false);
    });
  }
  test(`${name}: refuses a hardlinked config rather than splitting its identity`, (t) => {
    const { dir, path } = fixture(t);
    const second = join(dir, "alias.json");
    fs.linkSync(path, second);
    assert.throws(() => locus.registerLocusInMcpConfig(path, options), locus.LocusMcpConfigError);
    assert.equal(fs.readFileSync(path, "utf8"), original);
    assert.equal(fs.readFileSync(second, "utf8"), original);
  });
}
