#!/usr/bin/env node
// resolve-merge-conflicts.mjs — deterministic, lane-side merge-conflict resolver.
//
// Per-path policies applied to every unmerged file in the worktree:
//
//   CHANGELOG.md        union merge — keep both sides' hunks. Normally the
//                       `.gitattributes` rule `CHANGELOG.md merge=union` handles
//                       this inside git merge/rebase itself; the helper repeats
//                       it (git merge-file --union) so the resolution still
//                       works where the attribute is not in effect.
//   CLAUDE.md/README.md take the rebase/merge target's version (stage 2, "ours")
//                       and re-apply the incoming side's own hunks on top
//                       (git merge-file --ours): hunks that collide with the
//                       target side stay on the target's text, every other
//                       incoming hunk is applied.
//   Cargo.lock          restore the target side's lockfile, then regenerate —
//                       never hand-merge: `cargo metadata` performs Cargo's
//                       minimal re-resolution against the existing lockfile
//                       (no dependency added or upgraded; Cargo.toml is never
//                       touched), then `cargo metadata --locked` proves the
//                       result is frozen-consistent and every `source =` entry
//                       stays on crates.io.
//   bun.lock/bun.lockb  same shape via `bun install` + `bun install
//                       --frozen-lockfile`; package-lock.json via
//                       `npm install --package-lock-only` + `npm ci --dry-run`.
//   anything else       left conflicted — fail closed, human decides.
//
// GitHub's server-side merge ignores .gitattributes merge drivers, so
// `merge=union` only helps lane-side merges; run this script locally when a
// rebase stops. It never calls a model — resolution is pure git + the
// workspace's own package managers.
//
//   node scripts/resolve-merge-conflicts.mjs [--continue] [--dry-run] [-h]
//
//     --continue   resolve, stage, then `git rebase --continue` (or merge /
//                  cherry-pick --continue) and keep going until the operation
//                  finishes or a conflict it cannot resolve blocks it
//     --dry-run    print the policy each unmerged path would get; change nothing
//
// Exit 0: every conflict resolved (or none existed). Exit 1: unresolved
// conflicts remain — finish them by hand, then re-run or continue normally.
// Exit 2: usage error / not inside a git worktree. Never prints credentials.

import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, dirname, join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

const MARKER = /^(<{7}|>{7}|\|{7}|={7}$)/m;
const CRATES_IO = /^source = "(registry|sparse)\+https:[^"]*crates\.io/;
const NPMJS_URL = /^https:\/\/registry\.npmjs\.org\//;
const DOC_NAMES = new Set(['CLAUDE.md', 'README.md']);
const LOCK_CARGO = 'Cargo.lock';
const LOCK_BUN = new Set(['bun.lock', 'bun.lockb']);
const LOCK_NPM = 'package-lock.json';

// Classify a repo-relative path into a resolution policy, or null (fail closed).
export function classify(path) {
  const base = basename(path);
  if (base === 'CHANGELOG.md') return 'union';
  if (DOC_NAMES.has(base)) return 'doc-ours-plus-incoming';
  if (base === LOCK_CARGO) return 'cargo-lock';
  if (LOCK_BUN.has(base)) return 'bun-lock';
  if (base === LOCK_NPM) return 'npm-lock';
  return null;
}

function git(repo, args, opts = {}) {
  return spawnSync('git', args, {
    cwd: repo,
    encoding: opts.buffer ? 'buffer' : 'utf8',
    env: { ...process.env, GIT_EDITOR: 'true', GIT_SEQUENCE_EDITOR: 'true', ...opts.env },
  });
}

export function unmergedPaths(repo) {
  const r = git(repo, ['diff', '--name-only', '-z', '--diff-filter=U']);
  if (r.status !== 0) return [];
  return r.stdout.split('\0').filter(Boolean);
}

// path -> {1,2,3} stage presence from `git ls-files -u`.
function stages(repo) {
  const r = git(repo, ['ls-files', '-u']);
  const map = Object.create(null);
  for (const line of r.stdout.split('\n')) {
    const m = /^(\d+) ([0-9a-f]{40}) (\d)\t(.+)$/.exec(line);
    if (!m) continue;
    (map[m[4]] ??= {})[m[3]] = m[2];
  }
  return map;
}

function stageContent(repo, n, path) {
  const r = git(repo, ['show', `:${n}:${path}`], { buffer: true });
  return r.status === 0 ? r.stdout : null;
}

function restoreConflict(repo, path) {
  // Re-materialize the conflict markers in the worktree (index stages are
  // untouched until `git add`), so a failed resolution stays visibly conflicted.
  git(repo, ['checkout', '-m', '--', path]);
}

function mergeFile(repo, path, st, flag) {
  // Three-way file merge over index stages; result = ours (stage 2) merged with
  // the incoming side's (stage 3) hunks, conflict regions decided by `flag`.
  if (!st[2] || !st[3]) return { ok: false, reason: 'missing ours/incoming stage' };
  const tmp = mkdtempSync(join(tmpdir(), 'locus-merge-'));
  try {
    const f = (n) => join(tmp, n);
    for (const [n, s] of [['ours', 2], ['base', 1], ['theirs', 3]]) {
      const content = s === 1 && !st[1] ? Buffer.alloc(0) : stageContent(repo, s, path);
      if (content === null) return { ok: false, reason: `stage ${s} unreadable` };
      writeFileSync(f(n), content);
    }
    const r = git(repo, ['merge-file', '-p', flag, f('ours'), f('base'), f('theirs')]);
    if (r.status === null || r.status < 0) return { ok: false, reason: 'git merge-file failed' };
    if (MARKER.test(r.stdout)) return { ok: false, reason: 'conflict markers remained' };
    writeFileSync(join(repo, path), r.stdout);
    return { ok: true };
  } finally {
    rmSync(tmp, { recursive: true, force: true });
  }
}

function run(cmd, args, cwd) {
  return spawnSync(cmd, args, { cwd, encoding: 'utf8', env: process.env });
}

// Lockfile policy: restore "ours" (the base-side lockfile), let the package
// manager re-resolve only what the merged manifests require, then verify the
// result is frozen-consistent and registry-pinned. Manifests are never edited.
function resolveLockfile(repo, path, st, { regen, verify, checkSources }) {
  if (!st[2]) return { ok: false, reason: 'missing ours stage' };
  const abs = join(repo, path);
  const dir = dirname(abs);
  const back = () => restoreConflict(repo, path);
  if (git(repo, ['checkout', '--ours', '--', path]).status !== 0) {
    return { ok: false, reason: 'could not restore ours' };
  }
  const reg = regen(dir, abs);
  if (!reg.ok) { back(); return reg; }
  const ver = verify(dir, abs);
  if (!ver.ok) { back(); return ver; }
  const src = checkSources(abs);
  if (!src.ok) { back(); return src; }
  return { ok: true };
}

const cargoLock = {
  regen(dir, abs) {
    const r = run('cargo', ['metadata', '--format-version', '1', '--quiet'], dir);
    if (r.status !== 0) return { ok: false, reason: `cargo metadata: ${(r.stderr || '').trim().split('\n')[0]}` };
    let ws;
    try { ws = JSON.parse(r.stdout).workspace_root; } catch { return { ok: false, reason: 'cargo metadata output unreadable' }; }
    if (realpathSync(join(ws, 'Cargo.lock')) !== realpathSync(abs)) {
      return { ok: false, reason: 'not the workspace-root Cargo.lock — refusing to guess' };
    }
    return { ok: true };
  },
  verify(dir) {
    const r = run('cargo', ['metadata', '--locked', '--quiet'], dir);
    return r.status === 0
      ? { ok: true }
      : { ok: false, reason: `cargo metadata --locked: ${(r.stderr || '').trim().split('\n')[0]}` };
  },
  checkSources(abs) {
    const bad = readFileSync(abs, 'utf8').split('\n').filter((l) => l.startsWith('source =') && !CRATES_IO.test(l));
    return bad.length === 0
      ? { ok: true }
      : { ok: false, reason: `non-crates.io source in Cargo.lock: ${bad[0]}` };
  },
};

const bunLock = {
  regen(dir) {
    if (!existsSync(join(dir, 'package.json'))) return { ok: false, reason: 'no package.json beside bun.lock' };
    const r = run('bun', ['install'], dir);
    return r.status === 0 ? { ok: true } : { ok: false, reason: `bun install: ${(r.stderr || '').trim().split('\n')[0]}` };
  },
  verify(dir) {
    const r = run('bun', ['install', '--frozen-lockfile'], dir);
    return r.status === 0 ? { ok: true } : { ok: false, reason: 'bun install --frozen-lockfile failed' };
  },
  checkSources(abs) {
    if (basename(abs) === 'bun.lockb') return { ok: true }; // binary: integrity is bun-verified
    const bad = resolvedUrls(abs).filter((u) => !NPMJS_URL.test(u));
    return bad.length === 0 ? { ok: true } : { ok: false, reason: `non-npmjs resolved URL: ${bad[0]}` };
  },
};

const npmLock = {
  regen(dir) {
    if (!existsSync(join(dir, 'package.json'))) return { ok: false, reason: 'no package.json beside lockfile' };
    const r = run('npm', ['install', '--package-lock-only', '--ignore-scripts', '--no-audit', '--no-fund'], dir);
    return r.status === 0 ? { ok: true } : { ok: false, reason: `npm install --package-lock-only: ${(r.stderr || '').trim().split('\n')[0]}` };
  },
  verify(dir) {
    const r = run('npm', ['ci', '--dry-run', '--ignore-scripts', '--no-audit', '--no-fund'], dir);
    return r.status === 0 ? { ok: true } : { ok: false, reason: `npm ci --dry-run: ${(r.stderr || '').trim().split('\n')[0]}` };
  },
  checkSources(abs) {
    const bad = resolvedUrls(abs).filter((u) => !NPMJS_URL.test(u));
    return bad.length === 0 ? { ok: true } : { ok: false, reason: `non-npmjs resolved URL: ${bad[0]}` };
  },
};

function resolvedUrls(abs) {
  const text = readFileSync(abs, 'utf8');
  return [...text.matchAll(/"(?:resolved|registry)"\s*:\s*"(https?:[^"]+)"/g)].map((m) => m[1]);
}

export function resolveOne(repo, path, st) {
  switch (classify(path)) {
    case 'union':
      return mergeFile(repo, path, st, '--union');
    case 'doc-ours-plus-incoming':
      return mergeFile(repo, path, st, '--ours');
    case 'cargo-lock':
      return resolveLockfile(repo, path, st, cargoLock);
    case 'bun-lock':
      return resolveLockfile(repo, path, st, bunLock);
    case 'npm-lock':
      return resolveLockfile(repo, path, st, npmLock);
    default:
      return { ok: false, reason: 'no conflict policy for this path' };
  }
}

function opInProgress(repo) {
  const p = (n) => resolve(repo, git(repo, ['rev-parse', '--git-path', n]).stdout.trim());
  for (const [probe, op] of [
    ['rebase-merge', 'rebase'], ['rebase-apply', 'rebase'],
    ['MERGE_HEAD', 'merge'], ['CHERRY_PICK_HEAD', 'cherry-pick'],
  ]) {
    if (existsSync(p(probe))) return op;
  }
  return null;
}

function continueOp(repo, op) {
  const r = git(repo, [op, '--continue']);
  if (r.status === 0) return { ok: true };
  // A pick whose whole content merged away stops for confirmation; skip it
  // rather than resurrecting an empty commit. Guard on git's own empty-pick
  // message so a different stop never silently drops a commit.
  const msg = `${r.stderr || ''}\n${r.stdout || ''}`;
  if (op === 'rebase' && /now empty|--skip/.test(msg) && unmergedPaths(repo).length === 0) {
    const s = git(repo, ['rebase', '--skip']);
    if (s.status === 0) return { ok: true, skipped: true };
  }
  return { ok: false, reason: msg.trim() || `${op} --continue failed` };
}

export function resolveAll(repo, { dryRun = false } = {}) {
  const st = stages(repo);
  const results = { resolved: [], unresolved: [] };
  for (const path of unmergedPaths(repo)) {
    const policy = classify(path);
    if (dryRun) {
      results[policy ? 'resolved' : 'unresolved'].push({ path, policy: `plan: ${policy ?? 'none — manual'}` });
      continue;
    }
    const r = resolveOne(repo, path, st[path] ?? {});
    if (r.ok) {
      git(repo, ['add', '--', path]);
      results.resolved.push({ path, policy });
    } else {
      results.unresolved.push({ path, policy: `${policy ?? 'none'} (${r.reason})` });
    }
  }
  return results;
}

function printReport(results) {
  for (const { path, policy } of results.resolved) console.log(`  resolved  ${path}  (${policy})`);
  for (const { path, policy } of results.unresolved) console.log(`  conflict  ${path}  (${policy})`);
}

function usage() {
  console.log(`resolve-merge-conflicts.mjs -- deterministic lane-side conflict resolver

  node scripts/resolve-merge-conflicts.mjs [--continue] [--dry-run] [-h]

    --continue   resolve, stage, then rebase/merge/cherry-pick --continue until done
    --dry-run    print the policy each unmerged path would get; change nothing

  policies: CHANGELOG.md union; CLAUDE.md/README.md ours + incoming hunks;
  Cargo.lock/bun.lock/package-lock.json regenerated by the package manager;
  anything else stays conflicted. Exit 0 all resolved, 1 unresolved, 2 usage.`);
}

function main(argv) {
  const flags = { cont: false, dry: false };
  for (const a of argv) {
    if (a === '--continue') flags.cont = true;
    else if (a === '--dry-run') flags.dry = true;
    else if (a === '-h' || a === '--help') { usage(); process.exit(0); }
    else { console.error(`unknown argument: ${a}`); process.exit(2); }
  }
  const top = git('.', ['rev-parse', '--show-toplevel']);
  if (top.status !== 0) { console.error('not inside a git worktree'); process.exit(2); }
  const repo = top.stdout.trim();

  if (flags.dry) {
    const r = resolveAll(repo, { dryRun: true });
    if (r.resolved.length + r.unresolved.length === 0) console.log('no unmerged paths');
    printReport(r);
    process.exit(r.unresolved.length ? 1 : 0);
  }

  for (let i = 0; i < 1000; i++) {
    const r = resolveAll(repo);
    printReport(r);
    if (r.unresolved.length) {
      console.log(`\n${r.unresolved.length} conflict(s) left — resolve by hand, then re-run or continue.`);
      process.exit(1);
    }
    if (!flags.cont) {
      if (r.resolved.length === 0 && i === 0) console.log('no unmerged paths');
      process.exit(0);
    }
    const op = opInProgress(repo);
    if (!op) process.exit(0);
    const c = continueOp(repo, op);
    if (!c.ok) { console.error(`stop: ${op} could not continue: ${c.reason}`); process.exit(1); }
    if (opInProgress(repo) === null && unmergedPaths(repo).length === 0) process.exit(0);
    // operation still running — loop to resolve the next stop
  }
  console.error('stop: too many iterations');
  process.exit(1);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main(process.argv.slice(2));
}
