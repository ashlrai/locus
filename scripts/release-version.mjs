#!/usr/bin/env node
// One version for everything Locus ships. The Cargo workspace version is the
// source of truth; the carriers below must equal it, or npm and the GitHub
// release drift apart and `cargo build --locked` fails on a stale Cargo.lock.
//
//   check [--tag vX.Y.Z]   every carrier equals the Cargo version (and the tag)
//   version                print the Cargo version
//   sync [X.Y.Z]           write X.Y.Z (default: the Cargo version) to every carrier
//   next                   read commit subjects on stdin, print the next version
//
// Bump rule for `next` (the repo's merge subjects are `bridge(...)`, not
// conventional, so release-please would never find a release-worthy commit):
// breaking (`type!:` or BREAKING CHANGE) and `feat` raise the minor while the
// major is 0 (the major otherwise for breaking); everything else is a patch.
//
// No dependencies: text edits keep each file's formatting. Exits 1 on any
// mismatch; output never includes file contents beyond version strings.
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const SEMVER = /^(\d+)\.(\d+)\.(\d+)$/;
const CARGO_VERSION = /(\[workspace\.package\][^[]*?\nversion\s*=\s*")([^"]+)(")/;
const NPM_PACKAGES = ["npm/package.json", "npm-mcp/package.json"];
const SERVER_JSON = "server.json";

const read = (root, rel) => readFileSync(join(root, rel), "utf8");

export function cargoVersion(root) {
  const m = read(root, "Cargo.toml").match(CARGO_VERSION);
  if (!m) throw new Error("Cargo.toml: no [workspace.package] version");
  return m[2];
}

// Workspace crates are the Cargo.lock packages with no `source` line.
function lockBlocks(lock) {
  return lock.split(/(?=^\[\[package\]\]\n)/m).map((text) => ({
    text,
    name: text.match(/^name = "([^"]+)"/m)?.[1],
    version: text.match(/^version = "([^"]+)"/m)?.[1],
    local: /^\[\[package\]\]\n/.test(text) && !/^source = /m.test(text),
  }));
}

// [carrier label, current version] for every place a version is written.
export function carriers(root) {
  const out = [["Cargo.toml [workspace.package]", cargoVersion(root)]];
  for (const b of lockBlocks(read(root, "Cargo.lock"))) {
    if (b.local) out.push([`Cargo.lock ${b.name}`, b.version]);
  }
  for (const rel of NPM_PACKAGES) out.push([rel, JSON.parse(read(root, rel)).version]);
  const server = JSON.parse(read(root, SERVER_JSON));
  out.push([`${SERVER_JSON} version`, server.version]);
  (server.packages ?? []).forEach((p, i) => out.push([`${SERVER_JSON} packages[${i}] ${p.identifier}`, p.version]));
  return out;
}

export function check(root, tag) {
  const want = cargoVersion(root);
  const problems = [];
  if (!SEMVER.test(want)) problems.push(`Cargo version "${want}" is not X.Y.Z`);
  for (const [label, got] of carriers(root)) {
    if (got !== want) problems.push(`${label} is ${got}, Cargo is ${want}`);
  }
  if (tag !== undefined && tag !== `v${want}`) problems.push(`tag ${tag} is not v${want}`);
  return problems;
}

export function sync(root, version) {
  if (!SEMVER.test(version)) throw new Error(`"${version}" is not X.Y.Z`);
  const edit = (rel, fn) => {
    const before = read(root, rel);
    const after = fn(before);
    if (after !== before) writeFileSync(join(root, rel), after);
  };
  edit("Cargo.toml", (t) => t.replace(CARGO_VERSION, `$1${version}$3`));
  edit("Cargo.lock", (t) =>
    lockBlocks(t)
      .map((b) => (b.local ? b.text.replace(/^(version = ")[^"]+(")/m, `$1${version}$2`) : b.text))
      .join(""),
  );
  for (const rel of [...NPM_PACKAGES, SERVER_JSON]) {
    // Top-level and server.json packages[] versions: every `"version": "X"` at
    // the 2- or 6-space indent these files use, never a dependency range.
    edit(rel, (t) => t.replace(/^( {2}| {6})("version": ")\d+\.\d+\.\d+(")/gm, `$1$2${version}$3`));
  }
  return check(root);
}

export function nextVersion(current, subjects) {
  const m = current.match(SEMVER);
  if (!m) throw new Error(`"${current}" is not X.Y.Z`);
  const [major, minor, patch] = m.slice(1).map(Number);
  const lines = subjects.filter((s) => s.trim());
  if (lines.length === 0) return null;
  const breaking = lines.some((s) => /^\w+(\([^)]*\))?!:/.test(s) || /BREAKING[ -]CHANGE/.test(s));
  const feature = lines.some((s) => /^feat(\([^)]*\))?:/.test(s));
  if (breaking && major > 0) return `${major + 1}.0.0`;
  if (breaking || feature) return `${major}.${minor + 1}.0`;
  return `${major}.${minor}.${patch + 1}`;
}

function main(argv) {
  const root = process.env.RELEASE_VERSION_ROOT ?? fileURLToPath(new URL("..", import.meta.url));
  const [cmd, ...rest] = argv;
  const fail = (msg) => {
    console.error(msg);
    process.exit(1);
  };
  try {
    if (cmd === "version") return console.log(cargoVersion(root));
    if (cmd === "check") {
      const i = rest.indexOf("--tag");
      const problems = check(root, i >= 0 ? rest[i + 1] : undefined);
      if (problems.length) fail(problems.map((p) => `::error::version drift: ${p}`).join("\n"));
      return console.log(`all version carriers at ${cargoVersion(root)}`);
    }
    if (cmd === "sync") {
      const problems = sync(root, rest[0] ?? cargoVersion(root));
      if (problems.length) fail(problems.join("\n"));
      return console.log(`synced to ${cargoVersion(root)}`);
    }
    if (cmd === "next") {
      const subjects = readFileSync(0, "utf8").split("\n");
      const v = nextVersion(cargoVersion(root), subjects);
      return console.log(v ?? cargoVersion(root));
    }
    fail("usage: release-version.mjs check [--tag vX.Y.Z] | version | sync [X.Y.Z] | next < subjects");
  } catch (e) {
    fail(e.message);
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) main(process.argv.slice(2));
