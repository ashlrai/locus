// Run: node --test scripts/e2e.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const script = join(root, 'scripts', 'e2e.sh');
const locus = join(root, 'target', 'release', 'locus');

for (const failImport of [false, true]) {
  test(`graph fallback uses TMPDIR and cleans up ${failImport ? 'after import failure' : 'after import'}`, { timeout: 1_800_000 }, (t) => {
    const fixture = mkdtempSync(join(tmpdir(), 'locus-e2e-test-'));
    t.after(() => rmSync(fixture, { recursive: true, force: true }));
    const scratch = join(fixture, 'custom tmp');
    const bin = join(fixture, 'bin');
    const observed = join(fixture, 'graph-home');
    const imported = join(fixture, 'import-observed');
    mkdirSync(scratch);
    mkdirSync(bin);

    // Force both same-home attempts to fail, then observe the real fresh-home import.
    const wrapper = join(bin, 'locus');
    writeFileSync(wrapper, `#!/bin/bash
if [[ "\${1:-}" == graph && "\${2:-}" == import ]]; then
  if [[ "$LOCUS_HOME" != "$TMPDIR"/locus-graph-imp.* ]]; then
    exit 1
  fi
  [[ -d "$LOCUS_HOME" ]] || exit 1
  printf '%s' "$LOCUS_HOME" > "$GRAPH_HOME_RECORD"
  : > "$GRAPH_IMPORT_RECORD"
  [[ "$FAIL_GRAPH_IMPORT" != 1 ]] || exit 1
fi
exec '${locus.replaceAll("'", "'\\''")}' "$@"
`, { mode: 0o755 });

    const result = spawnSync('bash', [script], {
      cwd: root,
      env: {
        ...process.env,
        TMPDIR: scratch,
        LOCUS_BIN: wrapper,
        GRAPH_HOME_RECORD: observed,
        GRAPH_IMPORT_RECORD: imported,
        FAIL_GRAPH_IMPORT: failImport ? '1' : '0',
        LOCUS_GRAPH_PASSPHRASE: 'e2e-test',
      },
      encoding: 'utf8',
      timeout: 1_800_000,
      maxBuffer: 10 * 1024 * 1024,
    });

    assert.equal(result.signal, null, 'e2e process terminated unexpectedly');
    const stages = [...result.stdout.matchAll(/^==> (\d+[a-z]?)\./gm)];
    assert.equal(result.status, failImport ? 1 : 0,
      `unexpected e2e exit status at stage ${stages.at(-1)?.[1] ?? 'before startup'}`);
    assert.ok(existsSync(imported), 'fresh graph import was not reached');
    const graphHome = readFileSync(observed, 'utf8');
    assert.ok(dirname(graphHome) === scratch, 'graph home was not created directly under TMPDIR');
    assert.ok(/\/locus-graph-imp\.[^/]+$/.test(graphHome), 'unexpected graph home name');
    assert.equal(existsSync(graphHome), false, 'graph home was not cleaned up');
    assert.deepEqual(readdirSync(scratch), [], 'e2e left temporary directories behind');
    if (failImport) {
      assert.ok(/graph import failed/.test(result.stderr), 'expected graph import failure');
    } else {
      assert.ok(/ok  graph export\/import into fresh LOCUS_HOME/.test(result.stdout), 'fresh import did not pass');
      assert.ok(/e2e PASS/.test(result.stdout), 'e2e did not pass');
      const summary = result.stdout.match(/e2e PASS  \((\d+) checks, (\d+) skipped\)/);
      assert.ok(summary, 'e2e summary missing');
      t.diagnostic(`e2e: ${summary[1]} checks passed, ${summary[2]} skipped`);
    }
  });
}
