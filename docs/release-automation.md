# Release automation

Goal: every release-ready merge to `main` reaches GitHub Releases and npm
(`locus-cli`, `@ashlrai/locus-mcp`) with no human step, so npm never lags the
repo. At 0.6.2 npm matched the repo while 243+ commits sat unreleased, because
nothing bumped the version.

## Chain

```
merge to main ──► Release PR (bumps all carriers) ──► merge queue (CI Gate green)
   └─ release.yml: plan ─► build ─► GitHub Release + tag ─► npm publish (OIDC)
```

| Link | State | Where |
|------|-------|-------|
| `release.yml` auto-release on a Cargo version change | **Active** | `.github/workflows/release.yml` |
| One version across Cargo, lock, npm, `server.json` (`check` in CI Gate) | **Active** | `scripts/release-version.mjs` |
| Release PR opener + auto-merge | **Inactive template** | `docs/release-automation/release-pr.yml.template` |

Why not release-please: merge subjects here are `bridge(RQ-…)`, not
conventional commits, so release-please would never find a release-worthy
commit. `release-version.mjs next` bumps patch for any shipped-code merge, and
minor for `feat` or a breaking `!`.

## Owner action (needed to turn on the Release PR link)

The template is the first workflow that would write to this repo, so it is not
installed. It needs, in this order:

1. Security Hardening Engineer review of this change (release workflow trigger
   and the `plan` gate) and of the template.
2. A GitHub App `ashlr-release` installed on this repo only, with Contents and
   Pull requests write; repository variable `ASHLR_RELEASE_APP_ID` and secret
   `ASHLR_RELEASE_APP_KEY`. An App token is required: PRs and pushes made with
   `GITHUB_TOKEN` do not trigger CI, which would strand the PR in the queue.
3. Settings: allow auto-merge; keep CI Gate required in the merge-queue ruleset.
4. `git mv docs/release-automation/release-pr.yml.template .github/workflows/release-pr.yml`
   in a PR that is reviewed like any workflow change.
5. npm trusted publishers for both packages (owner `ashlrai`, repo `locus`,
   workflow `release.yml`, no environment) — already required by the existing
   publish job.

## Gate decision for the Security Hardening Engineer

A manual `v*` tag still needs a green **full-suite** `workflow_dispatch` CI run
on the sha. An automatic release accepts the **merge queue's CI Gate run** on
the sha instead (the full suite runs 6 AM / 6 PM ET and is not per-merge). If
that is too weak, require a dispatch run for both paths by deleting
`merge_group` from `events=` in the `plan` job.
