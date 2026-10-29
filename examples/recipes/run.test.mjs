// node --test examples/recipes
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..", "..");
const exe = process.platform === "win32" ? ".exe" : "";
const candidates = ["release", "debug"].map((p) => join(repo, "target", p, `locus${exe}`));
const built = candidates.find((p) => existsSync(p));
const run = (bin, recipe = join(here, "run.mjs")) => spawnSync(process.execPath, [recipe, "--locus", bin], { encoding: "utf8", timeout: 120_000 });

test("recipes pass against the workspace build", { skip: !built && "no target/{release,debug}/locus" }, () => {
  const r = run(built);
  assert.equal(r.status, 0, r.stdout + r.stderr);
  assert.match(r.stdout, /^COMPLETE: locus recipes \(4 checks\)$/m);
});

test("a missing binary is incomplete, never complete", () => {
  const r = run(join(repo, "target", "does-not-exist", `locus${exe}`));
  assert.equal(r.status, 2, r.stdout + r.stderr);
  assert.doesNotMatch(r.stdout, /^COMPLETE/m);
});

test("a missing recipe is reported and exits unsuccessfully", () => {
  const recipe = join(here, "does-not-exist.mjs");
  const r = run(process.execPath, recipe);
  assert.equal(r.status, 1);
  assert.match(r.stderr, /Cannot find module/);
  assert.ok(r.stderr.includes(recipe));
  assert.doesNotMatch(r.stdout, /^COMPLETE/m);
});
