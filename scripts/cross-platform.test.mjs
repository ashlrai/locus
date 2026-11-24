import assert from 'node:assert/strict';
import { test } from 'node:test';
import { homedir, platform } from 'node:os';
import { join } from 'node:path';
import { affectedPackages } from './fast-check.mjs';
import path from 'node:path';
import { cargoBinName, cargoHomeBin, locusBinName } from './setup.mjs';

test('workspace files select every crate and git backslashes still match', () => {
  assert.deepEqual(
    affectedPackages(['Cargo.toml', 'crates\\locus-core\\src\\lib.rs'], ['locus-core', 'locus-cli']),
    ['--workspace'],
  );
  assert.deepEqual(
    affectedPackages(['crates\\locus-cli\\src\\main.rs'], ['locus-core', 'locus-cli']),
    ['-p', 'locus-cli'],
  );
  assert.deepEqual(affectedPackages(['README.md'], ['locus-core']), []);
});

test('setup paths use the account home and a Windows exe suffix only on win32', () => {
  assert.equal(cargoHomeBin(), join(homedir(), '.cargo', 'bin'));
  const exe = platform() === 'win32';
  assert.equal(locusBinName(), exe ? 'locus.exe' : 'locus');
  assert.equal(cargoBinName(), exe ? 'cargo.exe' : 'cargo');
});

test('cargo home uses win32 separators for a Windows profile', () => {
  assert.equal(cargoHomeBin('C:\\Users\\dev', path.win32), 'C:\\Users\\dev\\.cargo\\bin');
  assert.equal(path.win32.delimiter, ';');
  assert.equal(cargoHomeBin('C:\\Users\\dev', path.win32).includes('/'), false);
});
