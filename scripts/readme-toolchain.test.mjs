import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

const read = (rel) => readFile(new URL(`../${rel}`, import.meta.url), 'utf8');
const normalized = (text) => text.split(/\s+/).join(' ');

const workspaceRustVersion = (manifest) =>
  manifest
    .split('[workspace.package]')[1]?.split('\n[')[0]
    ?.match(/^rust-version\s*=\s*"([^"]+)"/m)?.[1];

const workspaceMembers = (manifest) =>
  manifest
    .split('[workspace]')[1]?.split('\n[')[0]
    ?.match(/members\s*=\s*\[([^\]]+)\]/)?.[1]
    ?.match(/"([^"]+)"/g)
    ?.map((s) => s.slice(1, -1)) ?? [];

test('source installation documents the workspace toolchain and how to verify it', async () => {
  const [manifest, readme] = await Promise.all([read('Cargo.toml'), read('README.md')]);
  const version = workspaceRustVersion(manifest);
  assert.ok(version, 'workspace rust-version must be declared');

  const install = readme.split('### Install from source')[1]?.split('\n## ')[0];
  assert.ok(install, 'source installation section must exist');
  assert.ok(install.includes(`Rust ${version} or newer`), 'documented minimum must match the workspace');
  assert.match(install, /rustc --version/);
  assert.match(install, /cargo --version/);
  assert.match(install, /Rust 1\.83[\s\S]*idna_adapter[\s\S]*2024 edition/);
  assert.match(install, /cargo install\s+--git/);
});

test('every workspace member crate declares the same floor', async () => {
  const manifest = await read('Cargo.toml');
  const version = workspaceRustVersion(manifest);
  assert.ok(version, 'workspace rust-version must be declared');

  const members = workspaceMembers(manifest);
  assert.ok(members.length > 0, 'workspace members must be listed');
  for (const member of members) {
    const pkg = await read(`${member}/Cargo.toml`);
    const section = pkg.split('[package]')[1]?.split('\n[')[0];
    const inherits = /^rust-version\.workspace\s*=\s*true/m.test(section);
    const pinned = section?.match(/^rust-version\s*=\s*"([^"]+)"/m)?.[1] === version;
    assert.ok(inherits || pinned, `${member}/Cargo.toml must set rust-version.workspace = true or rust-version = "${version}"`);
  }
});

test('documented install requirements and wrapper fallbacks state the same floor', async () => {
  const version = workspaceRustVersion(await read('Cargo.toml'));
  assert.ok(version, 'workspace rust-version must be declared');

  const phrase = `Rust ${version} or newer`;
  for (const rel of ['npm/README.md', 'npm-mcp/README.md', 'integrations/homebrew/README.md']) {
    assert.ok(normalized(await read(rel)).includes(phrase), `${rel} must state: ${phrase}`);
  }
  for (const rel of ['npm/bin/locus.js', 'npm-mcp/bin/locus-mcp.js']) {
    const needle = `MIN_RUST_VERSION = "${version}"`;
    assert.ok((await read(rel)).includes(needle), `${rel} must carry ${needle} for its cargo-fallback messages`);
  }
});
