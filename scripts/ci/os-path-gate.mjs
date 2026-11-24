// Path gate for the Windows and macOS legs. Docs-only diffs do not match.
// Patterns are categories (path handling sources, shell, installers, manifests,
// lockfiles, this workflow), not a list of crate names.

import { appendFileSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const DOCS = /\.(md|markdown|svg|png|gif|webp|jpg|jpeg)$/i;

export const INSTALLER_PATTERNS = [
  /^scripts\//,
  /^npm\//,
  /^npm-mcp\//,
  /^integrations\/homebrew\//,
  /^\.github\/workflows\/ci\.yml$/,
  /^\.github\/workflows\/npm-wrapper-macos\.yml$/,
];

export const PLATFORM_PATTERNS = [
  ...INSTALLER_PATTERNS,
  /^crates\//,
  /(^|\/)Cargo\.toml$/,
  /(^|\/)Cargo\.lock$/,
  /(^|\/)package\.json$/,
  /(^|\/)package-lock\.json$/,
  /^rust-toolchain\.toml$/,
];

function matches(file, patterns) {
  if (!file || DOCS.test(file)) return false;
  return patterns.some((pattern) => pattern.test(file));
}

export function isInstallerPath(file) {
  return matches(file, INSTALLER_PATTERNS);
}

export function isPlatformPath(file) {
  return matches(file, PLATFORM_PATTERNS);
}

export function anyPlatformPath(files) {
  return files.some(isPlatformPath);
}

export function anyInstallerPath(files) {
  return files.some(isInstallerPath);
}

export function plan({ event, schedule, smokeCron, files }) {
  const platform = anyPlatformPath(files);
  const installer = anyInstallerPath(files);
  const pull = event === "pull_request" || event === "merge_group";
  const smokeDaily = event === "workflow_dispatch" || (event === "schedule" && schedule === smokeCron);
  return {
    fast_windows: pull && platform,
    fast_macos: pull && platform,
    smoke_linux: smokeDaily || (pull && installer),
    smoke_windows: smokeDaily || (pull && installer),
    smoke_macos: smokeDaily || (pull && installer),
  };
}

function writeOutputs(values) {
  const body = Object.entries(values).map(([key, value]) => `${key}=${value ? "true" : "false"}`).join("\n") + "\n";
  if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, body);
  else process.stdout.write(body);
}

function readFiles(text) {
  return text.split("\n").map((line) => line.trim()).filter(Boolean);
}

const isMain = process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1];

if (isMain && process.argv[2] === "plan") {
  const files = readFiles(readFileSync(0, "utf8"));
  writeOutputs(plan({
    event: process.env.EVENT ?? "",
    schedule: process.env.SCHEDULE ?? "",
    smokeCron: process.env.INSTALL_SMOKE_CRON ?? "",
    files,
  }));
}
