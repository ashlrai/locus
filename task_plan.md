# Task plan: RQ-PLAN-LOCUS

## Goal
Deliver an evidence-based, executable child-job decomposition for Locus as the swarm MCP and CLI broker, with a validated dependency graph and acceptance criteria.

## Phases
- [x] Inspect initial workspace and constraints.
- [x] Trace implemented broker boundaries and gaps.
- [x] Write child-job backlog and validation tests.
- [x] Run isolated checks and write notes.md.

## Decisions
- Leave the pre-existing .github/PULL_REQUEST_TEMPLATE.md modification untouched.
- This task produces a planning contract; it does not authorize provider execution or pin changes.

## Errors encountered
Initial format check found standard rustfmt layout differences in the new test; applied rustfmt; final format check passed.

## Status
Complete: nine child jobs, 22 distinct passing tests, workspace check/clippy/format passed, bridge notes written.

---

# SIEM export port
- [x] Inspect PR 433 and current main
- [x] Read failed policy check and port compatible changes
- [x] Run isolated tests, check, fmt and clippy
- [x] Record results in BRIDGE_NOTES.md

## RQ-LOCUSSIEM3Q8 validation progress
Workspace all-targets check, formatting, whitespace checks, wrapper tests (79), crypto tests (5), crypto inventory and JS/Ruby syntax checks pass. SIEM core tests (7), CLI tests (7), existing event tests (10) and workspace Clippy also pass. Complete: 108 successful tests, zero final failures.

## RQ-LOCUSSIEM3Q8 errors encountered
- Existing Locus session expired; public PR ref and check metadata were read without credentials. Raw logs returned HTTP 403; the crypto failure was reproduced locally.
- Local Node 22 does not accept the CI Node 20 directory test argument; explicit npm/test/*.test.js discovery ran all 79 tests successfully.
