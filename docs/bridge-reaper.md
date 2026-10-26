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
neither the main checkout, current working directory nor a locked worktree. Index entries marked assume-unchanged or skip-worktree also retain the worktree, because they can hide tracked edits from Git status.
Untracked **and ignored** files move to `~/.ashlr-runs/quarantine/<job>/`, retaining
relative paths. Symlinks move as links. Existing quarantine entries never get
overwritten, and moves across filesystems fail safely. If a later check fails,
already quarantined files remain recoverable there and the worktree is retained.
Only plain `git worktree remove` followed by `git worktree prune` performs removal. Each Git operation has a 60-second deadline. A prune failure is logged separately after successful removal; it never falsely reports the worktree as retained.

Every cycle appends disk capacity/used/free bytes before and after cleanup plus
per-job outcomes to `~/.ashlr-runs/worktree-reaper.jsonl`. No file contents, remote
credentials or Git stderr enter the log. A lock serializes reaper invocations.
Quarantine on the same volume preserves data but does not reclaim those files'
space; the log reports actual volume usage rather than claiming they were freed.

## Bridge dispatch with pinned native binaries

`scripts/bridge_cycle.py` supplies a runnable bridge dispatch boundary. It validates
both the native `locus` CLI and `locus-mcp` against absolute artifact paths and
SHA-256 digests on every dispatch. It rejects Node/shell scripts, symlinks, npm-cache
paths, special files, and changed digests. Use immutable, operator-owned release
files: a digest check cannot prevent a trusted installer replacing an artifact
between validation and execution.

For each Codex invocation it overrides only the named MCP server's command and
arguments:

```text
command = /absolute/pinned/locus
args = [mcp, --supervised, --mcp-bin, /absolute/pinned/locus-mcp]
```

This preserves existing environment and security settings and the supervised
Locus authority boundary. It does not create a pin or grant authority. Provider
access still requires a valid human pin. The server name must identify the existing **Locus** entry (including an alias);
overriding a different table would leave the old launcher active. Do not replace
a general Ashlr gateway entry that serves additional tools with a Locus-only server. Caller-supplied Codex `-c`/`--config` overrides are refused so they cannot
replace the launch table. Arguments after the second `--` are literal prompt text.

```bash
python3 scripts/bridge_cycle.py \
  --manifest /path/to/current-published-jobs.json \
  --worktree-root /Users/masonwyatt/Projects/ashlrai/wt \
  --locus-bin /absolute/pinned/locus --locus-sha256 <sha256> \
  --mcp-bin /absolute/pinned/locus-mcp --mcp-sha256 <sha256> \
  --server-name locus --cycles 1 -- -- 'job instructions'
```

The owning bridge can import `run_loop(dispatch, manifest, root, state, cycles=N)`
to refresh the manifest and select the next job in its dispatch callback. Reaping
runs in `finally` after every callback, including idle callbacks, failed child
exits, dispatch exceptions and interruption. A nonzero child status is retained.
`--idle` runs cleanup cycles without launching Codex. Bounded CLI batches are
useful for qualification; they repeat the supplied job, so ordinary scheduling
should use the callable with a job-selection callback. The standalone CLI does
not refresh PR metadata; stale manifests fail closed on every cycle.

`dispatch_codex(command, timeout=None)` owns a new process group for each Codex
session. On completion, failure, timeout or interruption it sends TERM to that
group, allows 0.5 seconds for shutdown, then sends KILL and reaps the child. It
never kills by process name. The CLI handles TERM and HUP through this cleanup
path and records the cycle before exiting with the conventional signal status.
Set `--session-timeout <seconds>` for an optional session deadline; no arbitrary
session deadline is imposed by default. The native Locus parent watcher also
covers the supervised intermediary and MCP processes when their parent exits.

## Integration and acceptance status

The owning live bridge loop is not present in this repository or the accessible
local source/install directories inspected for this task. Access to
`~/.ashlr-runs` is denied by the OS. The four exact target worktree paths were
absent from the local Locus, Phantom, Phantom Secrets, and plugin worktree
inventories. This change therefore supplies a tested executable integration but
does not claim it has been installed into the live scheduler or removed live
worktrees. The owning bridge must adopt this dispatch boundary before the live
“every bridge cycle” requirement is accepted.

Locus MCP access in this session has no usable supervised executor capability;
remote PR heads cannot be refreshed through it. Do not use cached local branch
heads as proof of the current remote PR head. The reaper intentionally keeps
jobs without fresh publication evidence, including `RQ-TESTDIET-phantom-secrets`.

Regression tests exercise twenty native stdio sessions and twenty npm-wrapper
sessions, checking termination after disconnect, plus twenty bridge dispatches
and disk records. These are local fixtures, not twenty authenticated live
`codex exec` sessions. Final live acceptance requires running those sessions
through the adopted bridge and checking that their owned `locus-mcp` descendants
are gone. Do not kill unrelated sessions to obtain a zero process count.

Run the isolated integration suites with:

```bash
HOME=$(mktemp -d) python3 -B -m unittest discover -s scripts -p 'test_bridge*.py' -v
```
