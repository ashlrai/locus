import test from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { plan } from './queue-recut-plan.mjs';

test('plans clean, stale and conflicting branches deterministically without changing the worktree', () => {
  const dir = mkdtempSync(join(tmpdir(), 'queue-plan-'));
  const git = (...args) => {
    const r = spawnSync('git', args, { cwd: dir, encoding: 'utf8', env: { ...process.env, GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null', GIT_AUTHOR_NAME: 'test', GIT_AUTHOR_EMAIL: 'test@example.test', GIT_COMMITTER_NAME: 'test', GIT_COMMITTER_EMAIL: 'test@example.test' } });
    assert.equal(r.status, 0, r.stderr);
    return r.stdout.trim();
  };
  const commit = (file, content) => { writeFileSync(join(dir, file), content); git('add', file); git('commit', '-qm', file); };
  try {
    git('init', '-qb', 'main');
    commit('shared', 'base\n');
    git('checkout', '-qb', 'conflict'); commit('shared', 'branch\n');
    git('checkout', '-qb', 'stale', 'main'); commit('other', 'branch\n');
    git('checkout', '-q', 'main'); commit('shared', 'main\n');
    git('checkout', '-qb', 'clean'); commit('clean', 'clean\n');
    const before = git('status', '--porcelain');
    const first = plan(dir, 'main', ['stale', 'conflict', 'clean']);
    assert.deepEqual(first.entries.map(({ branch, status }) => [branch, status]), [['clean', 'clean'], ['conflict', 'conflicting'], ['stale', 'stale']]);
    assert.deepEqual(first, plan(dir, 'main', ['clean', 'stale', 'conflict']));
    assert.equal(git('status', '--porcelain'), before);
    assert.throws(() => plan(dir, 'main', ['missing']));
    assert.throws(() => plan(dir, 'main', ['clean', 'clean']));
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
