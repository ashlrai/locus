# RQ-PLAN-LOCUS evidence

Baseline: c3ced98. No infrastructure access or credential resolution required.

- docs/swarm-broker.md and crates/locus-mcp/tests/http_swarm.rs: grant-to-node mapping, exact tool intersection, immutable snapshot, deny-all unmapped grants, audit fail-closed and revocation coverage already exist.
- docs/cli-routing.md and crates/locus-cli/tests/exec_for.rs: provider-specific trusted exec isolation exists; arbitrary CLI arguments are not an MCP scope/approval boundary.
- docs/credential-compatibility.md: phm: resolution deliberately unsupported pending a reviewed value-blind bridge. Published behavior cannot be inferred from this checkout.
- integrations/phantom/fleet-preflight.md: existing hub enforcement can be off/warn; new brokered dispatch must make its stronger requirement explicit without silently changing legacy callers.
- .github/CODEOWNERS reviewed read-only. No protected implementation changes are part of this planning delivery. Future jobs must obtain owner review and obey their own path restrictions.
- PLAN.md is historical architecture; GOALS.md contains implementation history. New backlog must point to current source evidence rather than repeat completed features.

---

# Findings
PR ref fetched without credentials from public GitHub. Locus session expired; no repin attempted.
Failed job 124431690333: Validate distribution wrappers failed (public job metadata and annotations read; raw log endpoint returns 403). Reproduced check-crypto failure on the port: siem_export.rs fixture ed25519:PUBLIC triggers classical-crypto inventory. Replaced it with CANARY, without changing policy.
Ported only exporter, CLI wiring, registration, and observability docs; no changelog/release edits. Shared filtering removes duplicate logic; default exports borrow the audit slice instead of cloning it. Added invalid-bound output preservation and timezone/filter-order CLI regressions.
Wrapper validation: 79/79 tests and 5/5 crypto tests pass; zero unallowlisted crypto files. Four JS syntax checks and Ruby syntax pass. Node 22 requires explicit npm/test/*.test.js discovery rather than CI Node 20 directory argument.
Final validation: workspace check/all-targets, Clippy/all-targets with warnings denied, fmt and diff checks passed. SIEM core 7/7, CLI 7/7, existing events 10/10 passed; total including wrappers and crypto is 108/108. No code changes were required after validation began.
