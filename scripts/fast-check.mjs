#!/usr/bin/env node
// Cross-platform local merge gate. Same checks as scripts/fast-check.sh.
// Git path lists use "/", including on Windows.
//
//   node scripts/fast-check.mjs

import { spawnSync } from 'node:child_process';
import { mkdtempSync, readdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const started = Date.now();

function finish(code) {
  const seconds = Math.round((Date.now() - started) / 1000);
  console.log(`Total: ${seconds}s`);
  process.exit(code);
}

function run(command, args) {
  return spawnSync(command, args, { cwd: root, encoding: 'utf8', env: process.env });
}

function step(label, command, args) {
  const result = run(command, args);
  if (result.status === 0) {
    console.log(`PASS ${label}`);
    return;
  }
  if (result.stdout) process.stdout.write(result.stdout);
  if (result.stderr) process.stderr.write(result.stderr);
  console.log(`FAIL ${label}`);
  finish(result.status || 1);
}

// Git prints paths with "/" even on Windows. A backslash is normalized so a
// core.quotepath or Windows git still matches crates/<name>/.
export function affectedPackages(paths, crateNames) {
  const normalized = paths.map((path) => path.replaceAll('\\', '/'));
  const all = normalized.some((path) =>
    path === 'Cargo.toml' ||
    path === 'Cargo.lock' ||
    path.startsWith('rust-toolchain') ||
    path.startsWith('.cargo/')
  );
  if (all) return ['--workspace'];
  const packages = [];
  for (const path of normalized) {
    for (const name of crateNames) {
      if (path === `crates/${name}` || path.startsWith(`crates/${name}/`)) {
        if (!packages.includes(name)) packages.push(name);
      }
    }
  }
  return packages.flatMap((name) => ['-p', name]);
}

function nulPaths(buffer) {
  return buffer.toString('utf8').split('\0').filter(Boolean);
}

function changedPaths() {
  const diff = spawnSync('git', ['diff', '--no-renames', '--name-only', '-z', 'origin/main', '--'], {
    cwd: root,
  });
  const untracked = spawnSync('git', ['ls-files', '--others', '--exclude-standard', '-z'], {
    cwd: root,
  });
  if (diff.status !== 0 || untracked.status !== 0) {
    console.log('FAIL changed-crates');
    finish(1);
  }
  return [...nulPaths(diff.stdout), ...nulPaths(untracked.stdout)];
}

function crateNames() {
  const crates = join(root, 'crates');
  return readdirSync(crates, { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .map((entry) => entry.name);
}

function main() {
  process.chdir(root);
  step('fmt', 'cargo', ['fmt', '--all', '--', '--check']);
  const packages = affectedPackages(changedPaths(), crateNames());
  if (packages.length) {
    step('clippy', 'cargo', ['clippy', '--locked', '--all-targets', ...packages, '--', '-D', 'warnings']);
    step('tests', 'cargo', ['test', '--locked', ...packages]);
  } else {
    console.log('PASS clippy (no changed crates)');
    console.log('PASS tests (no changed crates)');
  }
  finish(0);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
