# RQ-PLAN-LOCUS evidence

Baseline: c3ced98. No infrastructure access or credential resolution required.

- docs/swarm-broker.md and crates/locus-mcp/tests/http_swarm.rs: grant-to-node mapping, exact tool intersection, immutable snapshot, deny-all unmapped grants, audit fail-closed and revocation coverage already exist.
- docs/cli-routing.md and crates/locus-cli/tests/exec_for.rs: provider-specific trusted exec isolation exists; arbitrary CLI arguments are not an MCP scope/approval boundary.
- docs/credential-compatibility.md: phm: resolution deliberately unsupported pending a reviewed value-blind bridge. Published behavior cannot be inferred from this checkout.
- integrations/phantom/fleet-preflight.md: existing hub enforcement can be off/warn; new brokered dispatch must make its stronger requirement explicit without silently changing legacy callers.
- .github/CODEOWNERS reviewed read-only. No protected implementation changes are part of this planning delivery. Future jobs must obtain owner review and obey their own path restrictions.
- PLAN.md is historical architecture; GOALS.md contains implementation history. New backlog must point to current source evidence rather than repeat completed features.
