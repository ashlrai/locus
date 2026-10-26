# Published bridge worktree cleanup

`scripts/bridge_worktree_reaper.py` removes only these explicitly authorized jobs:
`RQ-CLI-1`, `RQ-DEADCODE-locus`, `RQ-PLUG-SELFTEST`, and `RQ-PS-AGENT-CREATE`.
`RQ-TESTDIET-phantom-secrets` is deliberately excluded.

The bridge must obtain fresh PR metadata using its authorized, Locus-scoped tool
access and atomically write a JSON list for the current cycle:

```json
[
  {
    "job": "RQ-CLI-1",
    "pr": {
      "url": "https://github.com/ashlrai/locus/pull/183",
      "head_oid": "<40-character PR head SHA>",
      "observed_at": 1793000000
    }
  }
]
```

`observed_at` is the Unix timestamp of the PR lookup, not the file creation time.
Metadata expires after five minutes. Never refresh its timestamp without another
lookup. The manifest is trusted bridge input, not proof of publication by itself;
the reaper does not access credentials or make hosting API requests. Missing PR
links, metadata errors, repository mismatch, or a head different from local HEAD
retain the worktree. Only inactive, published jobs belong in this manifest; the bridge must stop all
writers before publishing a job for reaping. A final file/head check narrows races,
but Git removal and the check cannot be atomic against an unrelated writer.

Call `run_cycle(manifest, worktree_root, state_dir)` from the bridge's `finally`
block on **every cycle**, including idle and failed dispatch cycles. Alternatively,
wrap each individual cycle command:

```bash
python3 scripts/bridge_worktree_reaper.py \
  --manifest /path/to/current-published-jobs.json \
  --worktree-root /Users/masonwyatt/Projects/ashlrai/wt \
  -- /path/to/bridge-single-cycle
```

The optional command runs first so it can refresh the manifest. Cleanup runs even
if the command exits unsuccessfully, and its exit status is preserved. Do not wrap
a long-running multi-cycle daemon: that would clean only when the daemon exits.

The state directory defaults to `~/.ashlr-runs`. Each eligible worktree must be a
direct child of the configured root, have no tracked edits or submodules, and be
neither the main checkout, current working directory nor a locked worktree.
Untracked **and ignored** files move to `~/.ashlr-runs/quarantine/<job>/`, retaining
relative paths. Symlinks move as links. Existing quarantine entries never get
overwritten, and moves across filesystems fail safely. If a later check fails,
already quarantined files remain recoverable there and the worktree is retained.
Only plain `git worktree remove` followed by `git worktree prune` performs removal.

Every cycle appends disk capacity/used/free bytes before and after cleanup plus
per-job outcomes to `~/.ashlr-runs/worktree-reaper.jsonl`. No file contents, remote
credentials or Git stderr enter the log. A lock serializes reaper invocations.
Quarantine on the same volume preserves data but does not reclaim those files'
space; the log reports actual volume usage rather than claiming they were freed.

## Integration status for RQ-LOCUS-SPAWN-LEAK

The external bridge cycle implementation is not part of the Locus repository.
This session could not inspect `~/.ashlr-runs` (OS `Operation not permitted`) or
process inventory. This change supplies a tested cycle hook and wrapper, but does
not claim that the live bridge invokes it or that the four live targets were
removed. Wire the hook into the owning bridge loop and collect its disk log before
accepting the live every-cycle requirement.

Run the isolated integration suite with:

```bash
HOME=$(mktemp -d) python3 -m unittest discover -s scripts -p 'test_bridge_worktree_reaper.py' -v
```
