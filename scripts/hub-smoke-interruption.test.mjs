// Run: node --test scripts/hub-smoke-interruption.test.mjs
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { setTimeout as delay } from 'node:timers/promises';
import { test } from 'node:test';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const source = readFileSync(join(root, 'scripts/hub-smoke.sh'), 'utf8');
const doc = readFileSync(join(root, 'docs/hub-smoke-interruption.md'), 'utf8');

test('interruption guide identifies the installed trap and its resources', () => {
  const traps = [...source.matchAll(/^\s*trap\s+(.+)$/gm)].map((match) => match[1]);
  assert.deepEqual(traps, ['cleanup EXIT'], 'review the guide if trap handling changes');
  assert.match(source, /cleanup\(\) \{ rm -rf -- "\$SMOKE_HOME" "\$SMOKE_PROJ" "\$SMOKE_ERR"; \}/);
  for (const name of ['SMOKE_HOME', 'SMOKE_PROJ', 'SMOKE_ERR']) {
    assert.ok(doc.includes(`| \`${name}\` |`), `missing resource ${name}`);
  }
  for (const detail of ['../scripts/hub-smoke.sh#L23-L40', '`EXIT`', '`INT`', '`TERM`',
    '`SIGKILL`', 'between `mktemp`', 'asynchronous', '`target/`']) {
    assert.ok(doc.includes(detail), `missing interruption detail ${detail}`);
  }
});

for (const signal of ['SIGTERM', 'SIGKILL']) {
  test(`${signal} ${signal === 'SIGTERM' ? 'runs cleanup' : 'can leave allocated paths'}`, { timeout: 10000 }, async (t) => {
    const fixture = mkdtempSync('/tmp/locus-interrupt-');
    t.after(() => rmSync(fixture, { recursive: true, force: true }));
    const scratch = join(fixture, 'temporary paths');
    mkdirSync(scratch);
    writeFileSync(join(scratch, 'keep'), 'unrelated');
    const marker = '# Prefer freshly built binaries over an older cargo install.';
    assert.ok(source.includes(marker));
    // Pause in a Bash builtin after all real allocations, without invoking Locus.
    const instrumented = source.replace(marker, 'printf ready > "$TMPDIR/ready"\nread -r ignored\n' + marker);
    const fixtureScript = join(fixture, 'smoke.sh');
    writeFileSync(fixtureScript, instrumented);
    const env = { ...process.env, TMPDIR: scratch };
    delete env.BASH_ENV;
    const child = spawn('bash', [fixtureScript], { env, stdio: ['pipe', 'ignore', 'ignore'] });
    t.after(() => { if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL'); });
    const exited = new Promise((resolve, reject) => {
      child.once('error', reject);
      child.once('exit', (code, exitSignal) => resolve({ code, signal: exitSignal }));
    });
    for (let i = 0; i < 200 && !existsSync(join(scratch, 'ready')); i++) {
      if (child.exitCode !== null || child.signalCode !== null) break;
      await delay(20);
    }
    assert.ok(existsSync(join(scratch, 'ready')), 'script did not reach interruption point');
    const allocated = readdirSync(scratch).filter((name) => name.startsWith('locus-hub-'));
    assert.equal(allocated.length, 3);
    assert.ok(child.kill(signal));
    const result = await exited;
    assert.equal(result.signal, signal);
    for (const name of allocated) {
      assert.equal(existsSync(join(scratch, name)), signal === 'SIGKILL', name);
    }
    assert.equal(readFileSync(join(scratch, 'keep'), 'utf8'), 'unrelated');
  });
}
