import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), 'utf8');

test('provider guides do not advertise the retired Phantom reveal resolver', () => {
  for (const path of ['docs/phantom-enrolled-repos.md', 'docs/PHANTOM-USAGE.md', 'docs/workers.md']) {
    const doc = read(path);
    assert.doesNotMatch(doc, /Locus resolves `phm:NAME` by running `phantom reveal/);
    assert.match(doc, /credential.compatibility\.md/);
  }
});

test('onboarding distinguishes supervised provider tools from control tools', () => {
  const doc = read('docs/onboarding.md');
  assert.match(doc, /control tools only/);
  assert.match(doc, /locus mcp --supervised/);
  assert.match(doc, /executor grant is bound to the original pin/);
});
