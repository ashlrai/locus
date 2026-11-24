// Install smoke. A downloaded payload is hashed against its sidecar before
// anything from it is executed. A missing or mismatched checksum stops the
// smoke and does not run the binary.

import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { basename } from "node:path";
import { fileURLToPath } from "node:url";

export const VERSION_TIMEOUT_MS = 120_000;

export function parseSha256Sidecar(text, filename) {
  const lines = text.trim().split(/\r?\n/).filter((line) => line.trim());
  if (lines.length !== 1) throw new Error("checksum sidecar must contain exactly one record");
  const match = lines[0].match(/^([0-9a-fA-F]{64})\s+\*?(\S+)$/);
  if (!match || match[2] !== filename) throw new Error("invalid checksum sidecar");
  return match[1].toLowerCase();
}

export function sha256File(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

// Writes the sidecar we just hashed, reads it back, and refuses to continue
// unless the bytes on disk still match. Callers must not spawn the file first.
export function verifyBeforeExecute(path) {
  const name = basename(path);
  const digest = sha256File(path);
  const sidecar = `${path}.sha256`;
  writeFileSync(sidecar, `${digest}  ${name}\n`);
  const expected = parseSha256Sidecar(readFileSync(sidecar, "utf8"), name);
  const actual = sha256File(path);
  if (expected !== actual) throw new Error(`SHA-256 mismatch for ${name}`);
  return digest;
}

export function versionLooksOk(output) {
  return /\d+\.\d+\.\d+/.test(output);
}

function quote(value) {
  if (!/[\s"]/.test(value)) return value;
  return `"${value.replaceAll('"', '\\"')}"`;
}

function run(cmd, args, opts = {}) {
  const shell = process.platform === "win32";
  const result = shell
    ? spawnSync([cmd, ...args].map(quote).join(" "), {
      encoding: "utf8",
      shell: true,
      cwd: opts.cwd,
      env: opts.env ?? process.env,
      timeout: opts.timeout,
      maxBuffer: 32 * 1024 * 1024,
    })
    : spawnSync(cmd, args, {
      encoding: "utf8",
      cwd: opts.cwd,
      env: opts.env ?? process.env,
      timeout: opts.timeout,
      maxBuffer: 32 * 1024 * 1024,
    });
  const output = `${result.stdout ?? ""}${result.stderr ?? ""}`;
  if (result.error || result.status !== 0) {
    const why = result.error ? result.error.message : `exit ${result.status}`;
    throw new Error(`${cmd} ${args.join(" ")}: ${why}\n${output.slice(-4000)}`);
  }
  return output;
}

function runVersion(bin) {
  verifyBeforeExecute(bin);
  const result = process.platform === "win32"
    ? spawnSync(`${quote(bin)} --version`, { encoding: "utf8", shell: true, timeout: VERSION_TIMEOUT_MS })
    : spawnSync(bin, ["--version"], { encoding: "utf8", timeout: VERSION_TIMEOUT_MS });
  const output = `${result.stdout ?? ""}${result.stderr ?? ""}`;
  if (result.error || result.status !== 0 || !versionLooksOk(output)) {
    const why = result.error ? result.error.message : `exit ${result.status}`;
    throw new Error(`locus --version: ${why}\n${output.slice(-2000)}`);
  }
  console.log(output.trim().split("\n").pop());
}

async function fetchBytes(url) {
  const headers = { "User-Agent": "locus-install-smoke" };
  if (process.env.GH_TOKEN) headers.Authorization = `Bearer ${process.env.GH_TOKEN}`;
  const response = await fetch(url, { headers });
  if (!response.ok) throw new Error(`${url} returned ${response.status}`);
  return Buffer.from(await response.arrayBuffer());
}

// Checksum sidecar first. The archive is downloaded only after the sidecar
// parses, and it is not extracted or executed unless the hash matches.
export async function downloadVerified(archiveUrl, filename, dest) {
  let sidecar;
  try {
    sidecar = (await fetchBytes(`${archiveUrl}.sha256`)).toString("utf8");
  } catch (error) {
    throw new Error(`checksum download failed; nothing downloaded or executed: ${error.message}`);
  }
  const expected = parseSha256Sidecar(sidecar, filename);
  const data = await fetchBytes(archiveUrl);
  const actual = createHash("sha256").update(data).digest("hex");
  if (actual !== expected) {
    throw new Error(`SHA-256 mismatch for ${filename}: expected ${expected}, got ${actual}`);
  }
  const { writeFileSync: write } = await import("node:fs");
  write(dest, data);
  return dest;
}

function hostBinary(cwd) {
  const name = process.platform === "win32" ? "locus.exe" : "locus";
  return `${cwd}/target/release/${name}`;
}

function installCommit(cwd) {
  run("cargo", ["build", "--release", "--locked", "-p", "locus-cli", "--bin", "locus"], { cwd });
  runVersion(hostBinary(cwd));
  const packed = ["npm", "npm-mcp"].map((dir) => {
    const text = run("npm", ["pack", "--silent"], {
      cwd: `${cwd}/${dir}`,
      env: { ...process.env, npm_config_update_notifier: "false" },
    }).trim().split("\n").pop();
    return `${cwd}/${dir}/${text}`;
  });
  run("npm", ["install", "-g", "--prefix", `${cwd}/.smoke-npm`, ...packed], {
    cwd,
    env: { ...process.env, LOCUS_NPM_NO_LINK: "1", npm_config_update_notifier: "false" },
  });
  console.log("npm wrapper installed with LOCUS_NPM_NO_LINK=1");
}

export function releaseFromApiBody(body) {
  const tag = body && typeof body.tag_name === "string" ? body.tag_name : "";
  if (!/^v\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)?$/.test(tag)) {
    throw new Error(`unexpected release tag: ${tag || "(missing)"}`);
  }
  return { tag, version: tag.slice(1) };
}

async function latestRelease(repo) {
  const headers = { Accept: "application/vnd.github+json", "User-Agent": "locus-install-smoke" };
  if (process.env.GH_TOKEN) headers.Authorization = `Bearer ${process.env.GH_TOKEN}`;
  const response = await fetch(`https://api.github.com/repos/${repo}/releases/latest`, { headers });
  if (!response.ok) throw new Error(`releases/latest returned ${response.status}`);
  return releaseFromApiBody(await response.json());
}

function hostTarget() {
  if (process.platform === "win32") return "x86_64-pc-windows-msvc";
  if (process.platform === "darwin") return process.arch === "arm64" ? "aarch64-apple-darwin" : "x86_64-apple-darwin";
  return "x86_64-unknown-linux-gnu";
}

async function installRelease(repo, cwd) {
  const { tag } = await latestRelease(repo);
  console.log(`latest published release: ${tag}`);
  const target = hostTarget();
  const ext = process.platform === "win32" ? "zip" : "tar.gz";
  const filename = `locus-${target}.${ext}`;
  const url = `https://github.com/${repo}/releases/download/${tag}/${filename}`;
  const dest = `${cwd}/${filename}`;
  await downloadVerified(url, filename, dest);
  console.log(`verified ${filename} from ${tag}; not executing an unverified archive`);
  run("cargo", ["install", "--git", `https://github.com/${repo}`, "--tag", tag, "locus-cli", "--locked", "--root", `${cwd}/.smoke-release`], { cwd });
  const binName = process.platform === "win32" ? "locus.exe" : "locus";
  runVersion(`${cwd}/.smoke-release/bin/${binName}`);
}

async function main() {
  const mode = process.argv[2];
  const cwd = process.cwd();
  if (mode === "commit") installCommit(cwd);
  else if (mode === "release") await installRelease(process.env.GITHUB_REPOSITORY, cwd);
  else throw new Error("usage: install-smoke.mjs commit|release");
}

const isMain = process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1];
if (isMain) {
  main().catch((error) => {
    console.error(error.message || error);
    process.exit(1);
  });
}
