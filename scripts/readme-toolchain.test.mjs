import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

test('source installation documents the workspace toolchain and how to verify it', async () => {
  const [manifest, readme] = await Promise.all([
    readFile(new URL('../Cargo.toml', import.meta.url), 'utf8'),
    readFile(new URL('../README.md', import.meta.url), 'utf8'),
  ]);
  const workspacePackage = manifest.split('[workspace.package]')[1]?.split('\n[')[0];
  const version = workspacePackage?.match(/^rust-version\s*=\s*"([^"]+)"/m)?.[1];
  assert.ok(version, 'workspace rust-version must be declared');

  const install = readme.split('### Install from source')[1]?.split('\n## ')[0];
  assert.ok(install, 'source installation section must exist');
  assert.ok(install.includes(`Rust ${version} or newer`), 'documented minimum must match the workspace');
  assert.match(install, /rustc --version/);
  assert.match(install, /cargo --version/);
  assert.match(install, /Rust 1\.83[\s\S]*idna_adapter[\s\S]*2024 edition/);
  assert.match(install, /cargo install\s+--git/);
});
