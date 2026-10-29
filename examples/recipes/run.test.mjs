// node --test examples/recipes
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..", "..");
const exe = process.platform === "win32" ? ".exe" : "";
const candidates = ["release", "debug"].map((p) => join(repo, "target", p, `locus${exe}`));
const built = candidates.find((p) => existsSync(p));
const run = (bin) => spawnSync(process.execPath, [join(here, "run.mjs"), "--locus", bin], { encoding: "utf8", timeout: 120_000 });

test("recipes pass against the workspace build", { skip: !built && "no target/{release,debug}/locus" }, () => {
  const r = run(built);
  assert.equal(r.status, 0, r.stdout + r.stderr);
  assert.match(r.stdout, /^COMPLETE: locus recipes \(4 checks\)$/m);
});

test("recipes resolve paths containing spaces", () => {
  assert.ok(built, "build locus before running the recipes tests");
  const root = mkdtempSync(join(tmpdir(), "locus recipes test "));
  try {
    const dir = join(root, "directory with spaces");
    mkdirSync(dir);
    const script = join(dir, "run recipes.mjs");
    const bin = join(dir, `locus binary${exe}`);
    copyFileSync(join(here, "run.mjs"), script);
    copyFileSync(built, bin);
    const r = spawnSync(process.execPath, [script, "--locus", bin], {
      cwd: dir,
      env: { ...process.env, TMPDIR: dir, TMP: dir, TEMP: dir },
      encoding: "utf8",
      timeout: 120_000,
    });
    assert.ifError(r.error);
    assert.equal(r.status, 0, "recipe runner failed in a directory containing spaces");
    assert.ok(/^COMPLETE: locus recipes \(4 checks\)$/m.test(r.stdout), "recipe runner did not complete all four checks");
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("a missing binary is incomplete, never complete", () => {
  const r = run(join(repo, "target", "does-not-exist", `locus${exe}`));
  assert.equal(r.status, 2, r.stdout + r.stderr);
  assert.doesNotMatch(r.stdout, /^COMPLETE/m);
});
