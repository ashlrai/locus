// Run: node --test scripts/hub-integration-test.test.mjs
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const script = join(dirname(fileURLToPath(import.meta.url)), 'hub-integration-test.sh');
const realMktemp = execFileSync('which', ['mktemp'], { encoding: 'utf8' }).trim();

for (const [failure, expectedStatus, expectedDirectories] of [
  ['init', 73, 2],
  ['project', 74, 1],
  ['home', 75, 0],
]) {
  test(`cleans up temporary directories after ${failure} failure`, (t) => {
    const root = mkdtempSync(join(tmpdir(), 'hub-integration-regression-'));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const scripts = join(root, 'scripts');
    const bin = join(root, 'bin');
    const debug = join(root, 'target', 'debug');
    const scratch = join(root, 'scratch');
    for (const dir of [scripts, bin, debug, scratch]) mkdirSync(dir, { recursive: true });
    copyFileSync(script, join(scripts, 'hub-integration-test.sh'));
    const executable = (path, contents) => writeFileSync(path, contents, { mode: 0o755 });
    executable(join(bin, 'node'), '#!/usr/bin/env bash\nexit 0\n');
    executable(join(bin, 'jq'), '#!/usr/bin/env bash\nexit 0\n');
    executable(join(bin, 'mktemp'), `#!/usr/bin/env bash
if [[ "$FAILURE" == home && "$*" == *locus-hub-int.XXXXXX ]]; then exit 75; fi
if [[ "$FAILURE" == project && "$*" == *locus-hub-int-proj.* ]]; then exit 74; fi
dir="$("$REAL_MKTEMP" "$@")" || exit $?
printf '%s\\n' "$dir" >> "$CREATED_DIRS"
printf '%s\\n' "$dir"
`);
    executable(join(debug, 'locus'), `#!/usr/bin/env bash
if [[ "$1" == init ]]; then
  : > "$INIT_MARKER"
  exit 73
fi
exit 0
`);
    const createdDirs = join(root, 'created-dirs');
    const initMarker = join(root, 'init-reached');
    const result = spawnSync('bash', [join(scripts, 'hub-integration-test.sh')], {
      env: {
        ...process.env,
        HOME: root,
        TMPDIR: scratch,
        PATH: `${bin}:${process.env.PATH}`,
        LOCUS_CONTROL_CAPABILITY: 'synthetic-test-capability',
        FAILURE: failure,
        REAL_MKTEMP: realMktemp,
        CREATED_DIRS: createdDirs,
        INIT_MARKER: initMarker,
      },
      encoding: 'utf8',
    });

    assert.equal(result.signal, null);
    assert.equal(result.status, expectedStatus);
    assert.equal(existsSync(initMarker), failure === 'init');
    const dirs = existsSync(createdDirs) ? readFileSync(createdDirs, 'utf8').trim().split('\n') : [];
    assert.equal(dirs.length, expectedDirectories);
    for (const dir of dirs) assert.equal(existsSync(dir), false, 'created directory was removed');
    assert.deepEqual(readdirSync(scratch), []);
  });
}
