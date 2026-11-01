import assert from 'node:assert/strict';
import { copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { test } from 'node:test';

function fixture(t) {
  const root = mkdtempSync(join(tmpdir(), 'locus-fast-check-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const home = join(root, 'home');
  mkdirSync(home);
  const env = { ...process.env, HOME: home,
    CARGO_HOME: process.env.CARGO_HOME || join(process.env.HOME, '.cargo'),
    RUSTUP_HOME: process.env.RUSTUP_HOME || join(process.env.HOME, '.rustup') };
  function run(command, args) {
    return spawnSync(command, args, { cwd: root, env, encoding: 'utf8' });
  }
  function git(...args) {
    const result = run('git', args);
    assert.equal(result.status, 0, result.stderr);
  }
  copyFileSync(new URL('../rust-toolchain.toml', import.meta.url), join(root, 'rust-toolchain.toml'));
  mkdirSync(join(root, 'scripts'));
  copyFileSync(new URL('./fast-check.sh', import.meta.url), join(root, 'scripts/fast-check.sh'));
  writeFileSync(join(root, '.gitignore'), '/home/\n/target/\n');
  writeFileSync(join(root, 'Cargo.toml'), '[workspace]\nmembers = ["crates/alpha", "crates/beta"]\nresolver = "2"\n');
  for (const name of ['alpha', 'beta']) {
    mkdirSync(join(root, `crates/${name}/src`), { recursive: true });
    writeFileSync(join(root, `crates/${name}/Cargo.toml`), `[package]\nname = "${name}"\nversion = "0.1.0"\nedition = "2021"\n`);
    writeFileSync(join(root, `crates/${name}/src/lib.rs`), 'pub fn value() -> u8 {\n    1\n}\n');
  }
  const lock = run('cargo', ['generate-lockfile']);
  assert.equal(lock.status, 0, lock.stderr);
  git('init', '-q');
  git('add', '.');
  git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-qm', 'fixture');
  git('update-ref', 'refs/remotes/origin/main', 'HEAD');
  return { root, run, git, check: () => run('bash', ['scripts/fast-check.sh']) };
}

test('clean temp copy passes formatting and avoids crate builds', (t) => {
  const { check } = fixture(t);
  const result = check();
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /PASS fmt\nPASS clippy \(no changed crates\)\nPASS tests \(no changed crates\)\nTotal: \d+s/);
});

test('seeded format error fails before clippy and tests', (t) => {
  const { root, check } = fixture(t);
  writeFileSync(join(root, 'crates/alpha/src/lib.rs'), 'pub fn value()->u8{1}\n');
  const result = check();
  assert.equal(result.status, 1);
  assert.match(result.stdout, /FAIL fmt\nTotal: \d+s/);
  assert.doesNotMatch(result.stdout, /PASS fmt|(?:PASS|FAIL) (?:clippy|tests)/);
});

test('changed crate is checked and a failing test blocks the gate', (t) => {
  const { root, check } = fixture(t);
  writeFileSync(join(root, 'crates/alpha/src/lib.rs'), '#[test]\nfn fails() {\n    assert_eq!(1, 2);\n}\n');
  const result = check();
  assert.equal(result.status, 1, result.stderr);
  assert.match(result.stdout, /PASS fmt\nPASS clippy/);
  assert.match(result.stdout, /FAIL tests\nTotal: \d+s/);
  assert.doesNotMatch(result.stderr, /Checking beta|Compiling beta/);
});

test('untracked crate source is included', (t) => {
  const { root, check } = fixture(t);
  writeFileSync(join(root, 'crates/beta/build.rs'), 'fn main() {\n    panic!("untracked build script");\n}\n');
  const result = check();
  assert.equal(result.status, 1);
  assert.match(result.stdout, /PASS fmt\nFAIL clippy\nTotal: \d+s/);
});

test('workspace changes check every crate', (t) => {
  const { root, check } = fixture(t);
  writeFileSync(join(root, 'Cargo.toml'), '[workspace]\nmembers = ["crates/alpha", "crates/beta"]\nresolver = "2"\n# shared change\n');
  const result = check();
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stderr, /Checking alpha/);
  assert.match(result.stderr, /Checking beta/);
  assert.match(result.stdout, /PASS tests\nTotal: \d+s/);
});

test('missing origin/main fails closed', (t) => {
  const { git, check } = fixture(t);
  git('update-ref', '-d', 'refs/remotes/origin/main');
  const result = check();
  assert.equal(result.status, 1);
  assert.match(result.stdout, /FAIL changed-crates\nTotal: \d+s/);
});
