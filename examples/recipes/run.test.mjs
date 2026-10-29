// node --test examples/recipes
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..", "..");
const exe = process.platform === "win32" ? ".exe" : "";
const candidates = ["release", "debug"].map((p) => join(repo, "target", p, `locus${exe}`));
const built = candidates.find((p) => existsSync(p));
const run = (bin, recipe = join(here, "run.mjs")) => spawnSync(process.execPath, [recipe, "--locus", bin], { encoding: "utf8", timeout: 120_000 });

test("help describes the invocation without running recipes", () => {
  const root = mkdtempSync(join(tmpdir(), "locus-recipes-help-"));
  try {
    // Recipe setup cannot create its temporary directory under a file.
    const unavailableTmp = join(root, "not-a-directory");
    writeFileSync(unavailableTmp, "");
    for (const flag of ["--help", "-h"]) {
      const r = spawnSync(process.execPath, [join(here, "run.mjs"), flag], {
        env: { ...process.env, TMPDIR: unavailableTmp, TMP: unavailableTmp, TEMP: unavailableTmp },
        encoding: "utf8",
        timeout: 120_000,
      });
      assert.ifError(r.error);
      assert.equal(r.status, 0);
      assert.match(r.stdout, /^Usage: node examples\/recipes\/run\.mjs \[--locus <absolute-path>\]\s*$/);
      assert.equal(r.stderr, "");
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("a recipe name is not a selector and returns usage", () => {
  const r = spawnSync(process.execPath, [join(here, "run.mjs"), "unknown-recipe"], { encoding: "utf8", timeout: 120_000 });
  assert.equal(r.status, 1);
  assert.match(r.stderr, /^Usage: node examples\/recipes\/run\.mjs \[--locus <absolute-path>\]/m);
  assert.doesNotMatch(r.stdout, /^(PASS|COMPLETE)/m);
});

test("an unrecognized option returns usage without running a recipe", () => {
  const r = spawnSync(process.execPath, [join(here, "run.mjs"), "--unknown-option"], { encoding: "utf8", timeout: 120_000 });
  assert.ifError(r.error);
  assert.equal(r.status, 1);
  assert.match(r.stderr, /^Usage: node examples\/recipes\/run\.mjs \[--locus <absolute-path>\]/m);
  assert.equal(r.stdout, "");
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

test("a missing recipe is reported and exits unsuccessfully", () => {
  const recipe = join(here, "does-not-exist.mjs");
  const r = run(process.execPath, recipe);
  assert.equal(r.status, 1);
  assert.match(r.stderr, /Cannot find module/);
  assert.ok(r.stderr.includes(recipe));
  assert.doesNotMatch(r.stdout, /^COMPLETE/m);
});
