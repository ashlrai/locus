# Plan queue recuts

Run `node scripts/queue-recut-plan.mjs BASE BRANCH...` using up-to-date local refs. It prints JSON with the base and branch commit IDs, status, and next action, sorted by branch name. Fetch refs before planning and recheck those IDs before acting; the plan is a snapshot and does not update branches or queue entries.

`clean` means the branch contains the base and merges cleanly. `stale` means the base is not an ancestor but the merge is clean; recut it on the base. `conflicting` means Git's merge simulation found conflicts; resolve them before recutting. Conflict takes precedence over staleness. Missing refs and Git errors fail the command. This plan does not bypass required checks: enqueue the resulting PR normally and allow the merge queue to run its required checks.
