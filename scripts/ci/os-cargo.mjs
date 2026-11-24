// Affected-only cargo check, build, and test for one OS leg.
// Stops at the first failure and prints that failure instead of the whole log.

import { spawnSync } from "node:child_process";
import { appendFileSync } from "node:fs";
import { dirname, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { reportFailure, stripAnsi } from "./fail-report.mjs";

const DOCS = /\.(md|markdown|svg|png|gif|webp|jpg|jpeg)$/i;

function posix(path) {
  return path.split(sep).join("/");
}

export function selectPackages(files, metadata) {
  const names = new Set(metadata.packages.map((pkg) => pkg.name));
  const byDir = [];
  const depsOf = new Map();
  for (const pkg of metadata.packages) {
    const dir = posix(relative(metadata.workspace_root, dirname(pkg.manifest_path)));
    byDir.push({ dir: dir === "" ? "." : dir, name: pkg.name });
    depsOf.set(pkg.name, pkg.dependencies.map((dep) => dep.name).filter((name) => names.has(name)));
  }
  const global = files.some((file) => file === "Cargo.toml" || file === "Cargo.lock" || file === "rust-toolchain.toml" || file.startsWith(".cargo/") || file.startsWith("rust-toolchain"));
  if (global) return [...names].sort();

  const hit = new Set();
  for (const file of files) {
    if (DOCS.test(file)) continue;
    let owner = null;
    for (const crate of byDir) {
      if (crate.dir === ".") continue;
      if (file === crate.dir || file.startsWith(`${crate.dir}/`)) {
        if (!owner || crate.dir.length > owner.dir.length) owner = crate;
      }
    }
    if (owner) hit.add(owner.name);
  }
  let grew = true;
  while (grew) {
    grew = false;
    for (const [name, deps] of depsOf) {
      if (hit.has(name)) continue;
      if (deps.some((dep) => hit.has(dep))) {
        hit.add(name);
        grew = true;
      }
    }
  }
  return [...hit].sort();
}

function cargoArgs(packages, metadata) {
  const every = packages.length === metadata.packages.length;
  return every ? ["--workspace", "--locked"] : [...packages.flatMap((name) => ["-p", name]), "--locked"];
}

function runCargo(args) {
  const result = spawnSync("cargo", args, {
    encoding: "utf8",
    maxBuffer: 32 * 1024 * 1024,
    env: { ...process.env, CARGO_TERM_COLOR: "never" },
  });
  const output = stripAnsi(`${result.stdout ?? ""}${result.stderr ?? ""}`);
  return { status: result.status, output, error: result.error };
}

function fail(output, status) {
  const reported = reportFailure(output, { summaryPath: process.env.GITHUB_STEP_SUMMARY });
  if (reported?.summary && process.env.GITHUB_STEP_SUMMARY) {
    appendFileSync(process.env.GITHUB_STEP_SUMMARY, reported.summary);
  }
  process.exit(status && status > 0 ? status : 1);
}

function changedFiles(event, base, head) {
  if (event !== "pull_request" && event !== "merge_group") return [];
  const baseCommit = spawnSync("git", ["merge-base", base, head], { encoding: "utf8" });
  if (baseCommit.status !== 0) {
    console.log(`::error::git merge-base failed for ${base} ${head}`);
    process.exit(1);
  }
  const diff = spawnSync("git", ["diff", "--name-only", baseCommit.stdout.trim(), head], { encoding: "utf8" });
  if (diff.status !== 0) {
    console.log("::error::git diff failed");
    process.exit(1);
  }
  return diff.stdout.split("\n").map((line) => line.trim()).filter(Boolean);
}

function metadata() {
  const result = spawnSync("cargo", ["metadata", "--no-deps", "--format-version", "1", "--locked"], {
    encoding: "utf8",
    maxBuffer: 32 * 1024 * 1024,
  });
  if (result.status !== 0) {
    fail(`${result.stdout ?? ""}${result.stderr ?? ""}`, result.status);
  }
  return JSON.parse(result.stdout);
}

function main() {
  const event = process.env.EVENT ?? "";
  const files = changedFiles(event, process.env.BASE_SHA ?? "", process.env.HEAD_SHA ?? "");
  const meta = metadata();
  const packages = selectPackages(files, meta);
  if (packages.length === 0) {
    console.log("no affected crates; skipping cargo check, build, and test");
    return;
  }
  console.log(`affected crates: ${packages.join(" ")}`);
  const selected = cargoArgs(packages, meta);
  for (const verb of ["check", "build", "test"]) {
    const extra = verb === "check" || verb === "build" ? ["--all-targets"] : [];
    const result = runCargo([verb, ...selected, ...extra]);
    if (result.error || result.status !== 0) fail(result.output, result.status);
    console.log(`cargo ${verb} ok (${packages.length} crate${packages.length === 1 ? "" : "s"})`);
  }
}

const isMain = process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1];
if (isMain) main();
