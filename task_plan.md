# Task plan: RQ-PLAN-LOCUS

## Goal
Deliver an evidence-based, executable child-job decomposition for Locus as the swarm MCP and CLI broker, with a validated dependency graph and acceptance criteria.

## Phases
- [x] Inspect initial workspace and constraints.
- [x] Trace implemented broker boundaries and gaps.
- [x] Write child-job backlog and validation tests.
- [x] Run isolated checks and write BRIDGE_NOTES.md.

## Decisions
- Leave the pre-existing .github/PULL_REQUEST_TEMPLATE.md modification untouched.
- This task produces a planning contract; it does not authorize provider execution or pin changes.

## Errors encountered
Initial format check found standard rustfmt layout differences in the new test; applied rustfmt; final format check passed.

## Status
Complete: nine child jobs, 22 distinct passing tests, workspace check/clippy/format passed, bridge notes written.
