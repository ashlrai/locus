// node --test examples/recipes
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, isAbsolute, join, relative, resolve } from "node:path";
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
  const root = mkdtempSync(join(tmpdir(), "locus-recipes-unknown-"));
  try {
    // Reject the argument before creating a recipe workspace or finding locus.
    const unavailableTmp = join(root, "not-a-directory");
    writeFileSync(unavailableTmp, "");
    const r = spawnSync(process.execPath, [join(here, "run.mjs"), "unknown-recipe"], {
      env: { ...process.env, PATH: "", TMPDIR: unavailableTmp, TMP: unavailableTmp, TEMP: unavailableTmp },
      encoding: "utf8",
      timeout: 120_000,
    });
    assert.ifError(r.error);
    assert.equal(r.status, 1);
    assert.match(r.stderr, /^Usage: node examples\/recipes\/run\.mjs \[--locus <absolute-path>\]\s*$/);
    assert.equal(r.stdout, "");
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("an unknown recipe name exits nonzero before any recipe runs", () => {
  const root = mkdtempSync(join(tmpdir(), "locus-recipes-name-"));
  try {
    // `acme` names a binding inside the recipes, but the runner has no recipe
    // selector: the name is rejected with the usage error and TMPDIR is left
    // untouched (no recipe workspace was created).
    const r = spawnSync(process.execPath, [join(here, "run.mjs"), "acme"], {
      env: { ...process.env, TMPDIR: root, TMP: root, TEMP: root },
      encoding: "utf8",
      timeout: 120_000,
    });
    assert.ifError(r.error);
    assert.notEqual(r.status, 0);
    assert.equal(r.status, 1);
    assert.match(r.stderr, /^Usage: node examples\/recipes\/run\.mjs \[--locus <absolute-path>\]\s*$/);
    assert.equal(r.stdout, "");
    assert.equal(readdirSync(root).length, 0, "a recipe workspace was created for an unknown recipe name");
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("an unrecognized option returns usage without running a recipe", () => {
  const r = spawnSync(process.execPath, [join(here, "run.mjs"), "--unknown-option"], { encoding: "utf8", timeout: 120_000 });
  assert.ifError(r.error);
  assert.equal(r.status, 1);
  assert.match(r.stderr, /^Usage: node examples\/recipes\/run\.mjs \[--locus <absolute-path>\]/m);
  assert.equal(r.stdout, "");
});

test("a missing --locus input returns usage before running a recipe", () => {
  const root = mkdtempSync(join(tmpdir(), "locus-recipes-missing-input-"));
  try {
    for (const args of [["--locus"], ["--locus", ""]]) {
      const r = spawnSync(process.execPath, [join(here, "run.mjs"), ...args], {
        env: { ...process.env, TMPDIR: root, TMP: root, TEMP: root },
        encoding: "utf8",
        timeout: 120_000,
      });
      assert.ifError(r.error);
      assert.equal(r.status, 1);
      assert.match(r.stderr, /^Usage: node examples\/recipes\/run\.mjs \[--locus <absolute-path>\]\s*$/);
      assert.equal(r.stdout, "");
      assert.deepEqual(readdirSync(root), [], "a recipe workspace was created for missing input");
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
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

test("a failing recipe command reports its exit and stops subsequent commands", () => {
  const root = mkdtempSync(join(tmpdir(), "locus-recipes-failure-"));
  try {
    const marker = join(root, "invocations");
    const fake = join(root, "locus");
    writeFileSync(fake, `#!${process.execPath}\nconst { appendFileSync } = require("node:fs");\nappendFileSync(${JSON.stringify(marker)}, JSON.stringify(process.argv.slice(2)) + "\\n");\nif (process.argv[4] === "acme") { console.log('PROBE {"gh":"fake-acme-token","aws":null,"tenant":"acme-corp","acme":null,"personal":null}'); } else { process.exit(7); }\n`);
    chmodSync(fake, 0o755);
    const r = run(fake);
    assert.ifError(r.error);
    assert.equal(r.status, 1);
    assert.match(r.stdout, /^PASS locus run -b acme:/m);
    assert.match(r.stdout, /^FAIL locus run -b personal exited 7, expected 0$/m);
    assert.doesNotMatch(r.stdout, /^(COMPLETE|SKIP|INCOMPLETE)/m);
    const calls = readFileSync(marker, "utf8").trim().split("\n").map(JSON.parse);
    assert.equal(calls.length, 2, "subsequent recipe commands must not run");
    assert.deepEqual(calls.map((call) => call.slice(0, 3)), [["run", "-b", "acme"], ["run", "-b", "personal"]]);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
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
    const marker = "requested recipe with spaces ran";
    const source = readFileSync(join(here, "run.mjs"), "utf8");
    assert.ok(source.includes("const args = process.argv.slice(2);"));
    writeFileSync(script, source.replace("const args = process.argv.slice(2);", `console.log(${JSON.stringify(marker)});\nconst args = process.argv.slice(2);`));
    copyFileSync(built, bin);
    const r = spawnSync(process.execPath, [script, "--locus", bin], {
      cwd: dir,
      env: { ...process.env, TMPDIR: dir, TMP: dir, TEMP: dir },
      encoding: "utf8",
      timeout: 120_000,
    });
    assert.ifError(r.error);
    assert.equal(r.status, 0, "recipe runner failed in a directory containing spaces");
    assert.ok(r.stdout.split(/\r?\n/).includes(marker), "the requested recipe was not executed");
    assert.ok(/^COMPLETE: locus recipes \(4 checks\)$/m.test(r.stdout), "recipe runner did not complete all four checks");
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("a Unicode recipe filename is opened and preserved in diagnostics", () => {
  const root = mkdtempSync(join(tmpdir(), "locus-recipes-unicode-"));
  try {
    const recipe = join(root, "レシピ-☕.mjs");
    const marker = "Unicode recipe opened";
    const source = readFileSync(join(here, "run.mjs"), "utf8");
    writeFileSync(recipe, source.replace("#!/usr/bin/env node\n", `#!/usr/bin/env node\nconsole.log(${JSON.stringify(marker)});\n`));

    const opened = spawnSync(process.execPath, [recipe, "--help"], { encoding: "utf8", timeout: 120_000 });
    assert.ifError(opened.error);
    assert.equal(opened.status, 0, opened.stderr);
    assert.ok(opened.stdout.split(/\r?\n/).includes(marker));
    assert.match(opened.stdout, /Usage: node examples\/recipes\/run\.mjs/);

    rmSync(recipe);
    const missing = run(process.execPath, recipe);
    assert.ifError(missing.error);
    assert.equal(missing.status, 1);
    assert.match(missing.stderr, /Cannot find module/);
    assert.ok(missing.stderr.includes(recipe), "diagnostic must preserve the Unicode path");
    assert.doesNotMatch(missing.stdout, /^COMPLETE/m);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("recipes run from a working directory outside the repository", { skip: !built && "no target/{release,debug}/locus" }, () => {
  const root = mkdtempSync(join(tmpdir(), "locus-recipes-cwd-"));
  try {
    // Recipe resolution must not consult the caller's cwd: run the repo's
    // runner from a directory that carries a workspace policy refusing every
    // recipe binding, with the recipe workspace in a separate sibling tmp.
    const outside = join(root, "outside-the-repo");
    const work = join(root, "recipe-tmp");
    mkdirSync(outside);
    mkdirSync(work);
    const rel = relative(repo, outside);
    assert.ok(rel.startsWith("..") || isAbsolute(rel), "the outside directory must not be inside the repository");
    const decoy = 'version = 1\ndefault_binding = "decoy"\nallowed_bindings = ["decoy"]\nrequire_pin = true\n';
    writeFileSync(join(outside, ".locus.toml"), decoy);
    const r = spawnSync(process.execPath, [join(here, "run.mjs"), "--locus", built], {
      cwd: outside,
      env: { ...process.env, TMPDIR: work, TMP: work, TEMP: work },
      encoding: "utf8",
      timeout: 120_000,
    });
    assert.ifError(r.error);
    assert.equal(r.status, 0, r.stdout + r.stderr);
    assert.match(r.stdout, /^COMPLETE: locus recipes \(4 checks\)$/m);
    assert.deepEqual(readdirSync(outside), [".locus.toml"], "the caller's working directory was written to");
    assert.equal(readFileSync(join(outside, ".locus.toml"), "utf8"), decoy, "the caller's workspace policy was modified");
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
