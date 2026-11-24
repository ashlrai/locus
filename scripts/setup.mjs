#!/usr/bin/env node
// Cross-platform development setup. Linux, macOS, and Windows.
// Idempotent and non-interactive. Installs Rust when it is missing, builds
// the CLI, and runs `locus --version` in a temporary LOCUS_HOME.
//
//   node scripts/setup.mjs
//
// scripts/setup.sh and scripts/setup.ps1 install Node when it is missing,
// then run this file.

import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, rmSync } from 'node:fs';
import { homedir, platform, arch, tmpdir } from 'node:os';
import path, { delimiter, dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

export const root = join(dirname(fileURLToPath(import.meta.url)), '..');

export function cargoBinName() {
  return platform() === 'win32' ? 'cargo.exe' : 'cargo';
}

export function locusBinName() {
  return platform() === 'win32' ? 'locus.exe' : 'locus';
}

export function cargoHomeBin(home = homedir(), paths = path) {
  return paths.join(home, '.cargo', 'bin');
}

function prependPath(dir) {
  const parts = (process.env.PATH || '').split(delimiter).filter(Boolean);
  if (!parts.includes(dir)) process.env.PATH = [dir, ...parts].join(delimiter);
}

function commandExists(name) {
  const finder = platform() === 'win32' ? 'where' : 'which';
  const result = spawnSync(finder, [name], { stdio: 'ignore' });
  return result.status === 0;
}

function run(command, args, env = process.env) {
  const result = spawnSync(command, args, { stdio: 'inherit', env });
  if (result.error) {
    console.error(`setup: could not run ${command}: ${result.error.message}`);
    process.exit(1);
  }
  if (result.status !== 0) process.exit(result.status || 1);
}

function ensureRust() {
  const cargoDir = cargoHomeBin();
  if (existsSync(join(cargoDir, cargoBinName()))) prependPath(cargoDir);
  if (!commandExists('cargo')) {
    console.log('setup: installing Rust with rustup');
    if (platform() === 'win32') {
      const init = join(tmpdir(), 'locus-rustup-init.exe');
      const rustupUrl = arch() === 'arm64'
        ? 'https://win.rustup.rs/aarch64'
        : 'https://win.rustup.rs/x86_64';
      run('powershell', [
        '-NoProfile',
        '-NonInteractive',
        '-Command',
        'Invoke-WebRequest -Uri $env:LOCUS_RUSTUP_URL -OutFile $env:LOCUS_RUSTUP_INIT',
      ], { ...process.env, LOCUS_RUSTUP_INIT: init, LOCUS_RUSTUP_URL: rustupUrl });
      run(init, ['-y', '--profile', 'minimal', '--component', 'rustfmt,clippy']);
    } else {
      run('bash', [
        '-lc',
        "curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --component rustfmt,clippy",
      ]);
    }
    prependPath(cargoDir);
  }
  if (commandExists('rustup')) run('rustup', ['component', 'add', 'rustfmt', 'clippy']);
}

function smoke() {
  console.log('setup: building locus');
  run('cargo', ['build', '-p', 'locus-cli']);
  const home = mkdtempSync(join(tmpdir(), 'locus-setup-'));
  try {
    const bin = join(root, 'target', 'debug', locusBinName());
    const result = spawnSync(bin, ['--version'], {
      stdio: 'inherit',
      env: { ...process.env, LOCUS_HOME: home },
    });
    if (result.status !== 0) process.exit(result.status || 1);
  } finally {
    rmSync(home, { recursive: true, force: true });
  }
  console.log('setup: smoke passed');
}

function main() {
  const host = platform();
  if (host !== 'linux' && host !== 'darwin' && host !== 'win32') {
    console.error(`setup.mjs supports Linux, macOS, and Windows (this host is ${host})`);
    process.exit(1);
  }
  process.chdir(root);
  ensureRust();
  smoke();
  console.log(`setup: cargo ${commandExists('cargo') ? 'ready' : 'missing'}`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
