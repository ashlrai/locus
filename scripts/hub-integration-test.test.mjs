// Run: node --test scripts/hub-integration-test.test.mjs
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-integration-test.sh');
const realMktemp = execFileSync('which', ['mktemp'], { encoding: 'utf8' }).trim();

for (const scenario of ['project-creation', 'init']) {
  test(`cleans temporary directories after ${scenario} failure`, (t) => {
    const root = mkdtempSync(join(tmpdir(), 'hub-integration-cleanup-'));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const bin = join(root, 'bin');
    const scratch = join(root, 'scratch');
    const markers = join(root, 'markers');
    mkdirSync(bin);
    mkdirSync(scratch);
    mkdirSync(markers);
    writeFileSync(join(bin, 'mktemp'), `#!/usr/bin/env bash
set -eu
case "$2" in
  */locus-hub-int.XXXXXX)
    dir="$("$REAL_MKTEMP" "$@")"
    : > "$MARKERS/home-created"
    if [[ "$SCENARIO" == init ]]; then : > "$dir/bindings"; fi
    printf '%s\\n' "$dir"
    ;;
  */locus-hub-int-proj.XXXXXX)
    if [[ "$SCENARIO" == project-creation ]]; then
      : > "$MARKERS/project-failed"
      exit 1
    fi
    dir="$("$REAL_MKTEMP" "$@")"
    : > "$MARKERS/project-created"
    printf '%s\\n' "$dir"
    ;;
  *) exec "$REAL_MKTEMP" "$@" ;;
esac
`, { mode: 0o755 });
    const result = spawnSync('bash', [script], {
      env: { ...process.env, TMPDIR: scratch, PATH: `${bin}:${process.env.PATH}`,
        REAL_MKTEMP: realMktemp, MARKERS: markers, SCENARIO: scenario },
      encoding: 'utf8',
    });

    assert.equal(result.signal, null);
    assert.notEqual(result.status, 0);
    assert.deepEqual(readdirSync(markers).sort(), [
      'home-created', scenario === 'init' ? 'project-created' : 'project-failed',
    ]);
    if (scenario === 'init') {
      assert.ok(result.stdout.includes('== hub-integration-test LOCUS_HOME='));
      assert.match(result.stderr, /file exists/i);
    }
    assert.deepEqual(readdirSync(scratch), []);
  });
}
