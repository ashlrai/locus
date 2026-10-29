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
const run = (bin) => spawnSync(process.execPath, [join(here, "run.mjs"), "--locus", bin], { encoding: "utf8", timeout: 120_000 });

test("a recipe name is not a selector and returns usage", () => {
  const r = spawnSync(process.execPath, [join(here, "run.mjs"), "unknown-recipe"], { encoding: "utf8", timeout: 120_000 });
  assert.equal(r.status, 1);
  assert.match(r.stderr, /^Usage: node examples\/recipes\/run\.mjs \[--locus <absolute-path>\]/m);
  assert.doesNotMatch(r.stdout, /^(PASS|COMPLETE)/m);
});

test("arguments after an absolute --locus path are ignored", () => {
  const r = spawnSync(process.execPath, [join(here, "run.mjs"), "--locus", join(repo, "target", "does-not-exist", `locus${exe}`), "unknown-recipe"], { encoding: "utf8", timeout: 120_000 });
  assert.equal(r.status, 2);
  assert.match(r.stdout, /^SKIP .* is not installed/m);
  assert.match(r.stdout, /^INCOMPLETE: locus recipes$/m);
});

test("a failed check stops the run without reporting completion", () => {
  // Node is an executable but not a Locus binary, so the first check fails.
  const r = run(process.execPath);
  assert.equal(r.status, 1);
  assert.match(r.stdout, /^FAIL /m);
  assert.doesNotMatch(r.stdout, /^(PASS|COMPLETE)/m);
});

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
