#!/usr/bin/env node
// Read-only queue planning against locally available refs. Output is stable JSON.
import { spawnSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';

function git(repo, args, allowed = [0]) {
  const r = spawnSync('git', args, { cwd: repo, encoding: 'utf8' });
  if (!allowed.includes(r.status)) throw new Error(`git ${args[0]} failed`);
  return r;
}

export function plan(repo, baseRef, branches) {
  const sha = (ref) => git(repo, ['rev-parse', '--verify', '--end-of-options', `${ref}^{commit}`]).stdout.trim();
  const base = sha(baseRef);
  const entries = branches.map((branch) => {
    const head = sha(branch);
    const ancestor = git(repo, ['merge-base', '--is-ancestor', base, head], [0, 1]).status === 0;
    // merge-tree does not alter the index or worktree. Errors are not conflicts.
    const merge = git(repo, ['merge-tree', '--write-tree', base, head], [0, 1]);
    const status = merge.status === 1 ? 'conflicting' : ancestor ? 'clean' : 'stale';
    return { branch, head, status, action: status === 'clean' ? 'none' : status === 'stale' ? 'recut' : 'resolve-conflicts' };
  }).sort((a, b) => a.branch < b.branch ? -1 : a.branch > b.branch ? 1 : 0);
  if (new Set(entries.map((entry) => entry.branch)).size !== entries.length) throw new Error('duplicate branch');
  return { base, entries };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const [base, ...branches] = process.argv.slice(2);
    if (!base || !branches.length) throw new Error('usage: queue-recut-plan.mjs BASE BRANCH...');
    console.log(JSON.stringify(plan('.', base, branches), null, 2));
  } catch (error) { console.error(error.message); process.exitCode = 2; }
}
