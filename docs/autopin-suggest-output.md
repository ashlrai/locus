# `locus autopin suggest` output

`locus autopin suggest` reads local pin history and reports a possible binding
for the current Git repository. It does not pin or change a workspace. The
examples below use synthetic aliases and timestamps; run with `--json` for the
machine-readable form. An empty result is a successful command, not an error.

## A suggestion

With four matching pins for `acme` and one for `personal`, the command emits:

<!-- suggested-json -->
```json
{
  "schema": "locus.autopin.suggest.v1",
  "applied": false,
  "report": {
    "schema": "locus.autopin.suggest.v1",
    "history_enabled": true,
    "repo_detected": true,
    "matched_entries": 5,
    "suggestion": "acme",
    "reason": "suggested",
    "min_score": 3,
    "candidates": [
      {"alias": "acme", "pins": 4, "mismatches": 0, "score": 4, "last_seen": "2026-10-25T00:00:04Z"},
      {"alias": "personal", "pins": 1, "mismatches": 0, "score": 1, "last_seen": "2026-10-25T00:00:05Z"}
    ]
  },
  "written": null
}
```

Without `--json`, candidate rows precede `suggested binding: acme` and an
instruction to use `locus autopin suggest --apply`. Counts and timestamps
reflect local history, so actual output varies.

## No suggestion

With a remote but no matching history, the result has no candidates:

<!-- empty-json -->
```json
{
  "schema": "locus.autopin.suggest.v1",
  "applied": false,
  "report": {
    "schema": "locus.autopin.suggest.v1",
    "history_enabled": true,
    "repo_detected": true,
    "matched_entries": 0,
    "suggestion": null,
    "reason": "no_history",
    "min_score": 3,
    "candidates": []
  },
  "written": null
}
```

The human-readable message is `no pin history for this repo yet; nothing to suggest`.
No suggestion does not always mean an empty candidate list. Two matching pins
produce `thin_evidence`, for example:

<!-- thin-json -->
```json
{
  "schema": "locus.autopin.suggest.v1",
  "applied": false,
  "report": {
    "schema": "locus.autopin.suggest.v1",
    "history_enabled": true,
    "repo_detected": true,
    "matched_entries": 2,
    "suggestion": null,
    "reason": "thin_evidence",
    "min_score": 3,
    "candidates": [
      {"alias": "acme", "pins": 2, "mismatches": 0, "score": 2, "last_seen": "2026-10-25T00:00:02Z"}
    ]
  },
  "written": null
}
```

Human output includes the candidate row and `not enough evidence yet (need a
score of at least 3); no suggestion`.

## Reading the report

Each candidate's `score` is `pins - mismatches`; a mismatch counts against the
alias that was pinned. A suggestion requires a top score of at least
`min_score` (currently 3) and a strictly higher score than the next candidate.
Candidates are ordered by score, pins and last-seen timestamp descending, then
alias ascending. `last_seen` is the latest matching pin or mismatch timestamp.
`matched_entries` counts all matching events, including events that do not
contribute to candidates and entries with invalid aliases. Thus `no_history`
can also have a nonzero `matched_entries` when no eligible candidate remains.

| `reason` | Meaning |
| --- | --- |
| `suggested` | The leading candidate meets the threshold and has a strict lead. |
| `history_disabled` | Local history is disabled. `repo_detected` is false because the command returns before checking remotes. |
| `no_remote` | No Git remote was found, including when Git cannot inspect the directory. |
| `no_history` | There is no history key or no eligible candidate among matching entries. |
| `thin_evidence` | The top score is below the threshold, even if candidates tie. |
| `tied` | The top score meets the threshold but is shared by multiple candidates. |

History is local and enabled by default; `[history] enabled = false` in the
Locus config or `LOCUS_PIN_HISTORY=0` disables it. The environment switch takes
precedence over the config. Suggestion matching uses the normalized, keyed hash
of `origin` when present, otherwise all remotes. The command does not print raw
remote URLs. Remotes are controlled by the repository and are not authenticated:
a spoofed remote can match another repository's history. Review the alias and
workspace before applying a suggestion. See the [autopin review checklist](autopin-review-checklist.md).

## Applying or previewing

`locus autopin suggest --apply` recomputes the suggestion and requires the
operator control capability. It validates the binding and workspace policy,
then writes `default_binding` to the appropriate `.locus.toml`; it does not
pin. On success, JSON uses the same envelope with `applied: true` and `written`
set to the path, while human output names the binding and file. If there is no
suggestion, apply fails and writes nothing. Other preflight and filesystem
errors can also prevent application; a directory-sync error can be reported
after the file has been renamed.

`locus autopin suggest --apply --dry-run --json` performs read-only preflight
and emits a `locus.plan.v1` plan containing the report, instead of the normal
suggestion envelope. A plan is not authorization and does not guarantee the
actual apply will succeed. See [dry-run limits](README.md#limits-and-unsupported-operations).
