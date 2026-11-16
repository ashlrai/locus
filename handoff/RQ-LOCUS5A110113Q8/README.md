# Security handoff — locus PR #626 (RQ-LOCUS5A110113Q8)

PR #626 touches `.github/workflows/*`, so publish.sh must refuse it and the
change is staged here for Security review instead of being pushed to the fork.

- Staged ref (in repo `.git`): `refs/heads/staged-drain/RQ-LOCUS5A110113Q8`
- Commit: `e29c12a50443805a0bd1c6f7a7884c61e74b0024` (parent = origin/main)
- Tree: `a9492a5dbd6b5cd0ab67c15cbb786d9320e0685f`
- Original PR head: `b66de80d7acbad651ba168798e1aad9a2743346e` (merge-base `17f684a`)
- `branch.diff` is `git diff origin/main staged-drain/RQ-LOCUS5A110113Q8`.

## Resolved conflicts (merge vs origin/main)

- `CONTRIBUTING.md`: resolved to main's content (`locus-install.XXXXXX`
  mktemp name wins); PR's intent was not the dir name.
- `docs/temp-parent-reference.md`: union — main's sentence + PR's added clause
  about conformance running the smoke under a spaced TMPDIR.
- `scripts/fast-check.sh` and `docs/hub-smoke-prerequisites.md` merge to
  main's content (no residual delta).

## What the residual diff does (3 files)

- `.github/workflows/ci.yml`: adds step "Hub scripts temporary-path
  discipline" — runs 10 existing `scripts/*.test.mjs` files with
  `node --test`. No new actions, permissions, secrets, or triggers.
- `.github/workflows/conformance.yml`: adds step "Hub contract smoke (TMPDIR
  with spaces)" — runs `./scripts/hub-smoke.sh` with `TMPDIR` containing
  spaces and glob chars, asserts the dir is left empty. `run:` only.
- `docs/temp-parent-reference.md`: one sentence noting the conformance step.

Rule check: no new third-party actions, no permissions/id-token changes, no
`secrets.*` refs, no `pull_request_target`/`workflow_run`, no new triggers.
YAML not machine-validated (no pyyaml in sandbox); diff inspected by hand.
