# Locus swarm broker: child jobs

Parent **RQ-PLAN-LOCUS**, baseline **c3ced98**. This is a proposed backlog, not
queued jobs or an authorization to mint grants, re-pin, resolve secrets or deploy.
The [machine-readable job records](swarm-child-jobs.json) are the canonical
scope, dependencies, acceptance criteria and test commands. All children target
`locus`, pool `codex-personal`, tier `frontier`; downstream changes require their
own repository jobs. IDs are stable handoff identifiers, not confirmed router IDs.

## Outcome and current boundary

Every swarm job should reach MCP and approved CLI operations through authority
bound to its agent and tenant, with no ambient account fallback. Phantom owns
secret delivery; Locus owns identity, scope and execution admission; the router
owns scheduling and job lifecycle. Agents never acquire operator pin authority.

The [swarm broker](swarm-broker.md) already implements exact tool grants, sealed
sessions, per-grant isolation, immutable graph snapshots and fail-closed auditing.
The [CLI route](cli-routing.md) already isolates a selected provider, but native
arguments remain trusted and do not inherit MCP scope or approval checks.
[Phantom resolution](credential-compatibility.md) is deliberately unsupported.
These are the starting boundaries, not features this plan claims to implement.

## Dependency-ordered dispatch

| ID | Priority | Deliverable | Prerequisites |
| --- | --- | --- | --- |
| RQ-LOCUS-01 | P0 | Versioned dispatch/readiness contract | None; start here |
| RQ-LOCUS-02 | P0 | Credential-free graph preflight | 01 |
| RQ-LOCUS-03 | P0 | Phantom scoped bridge specification and mock conformance | 01; external owner for activation |
| RQ-LOCUS-04 | P0 | Restricted grant-scoped CLI operations | 01 |
| RQ-LOCUS-05 | P0 | Cancellation, expiry and owned-resource cleanup | 02, 04 |
| RQ-LOCUS-06 | P1 | Per-agent resource limits and fairness | 02 |
| RQ-LOCUS-07 | P1 | Job correlation and execution outcomes | 01, 04 |
| RQ-LOCUS-08 | P0 | Router integration harness and adoption handoff | 02, 03, 04, 05; router and Phantom owners |
| RQ-LOCUS-09 | P0 | End-to-end conformance and rollout evidence | 05, 06, 07, 08 |

After 01, jobs 02, 03 and 04 are logically independent. Actual concurrent edits
must use isolated worktrees and coordinate overlapping adapter/store/transport
paths. The critical production path includes the external Phantom bridge and
router adoption: merging a mock contract does not satisfy those gates. P1 jobs
06 and 07 are required before the final rollout despite their lower urgency.

Each child should deliver one reviewable vertical slice. If a contract proves
larger than one slice, split it before dispatch and preserve explicit dependency
edges. Do not expand into scheduler replacement, vault implementation, new cloud
adapters, general shell sandboxing or a new remote transport.

## Invariant ownership

| AGENTS.md invariant | Primary jobs and proof |
| --- | --- |
| 1 Sealed pin; 7 agents cannot pin | 01, 05: authenticated dispatch, replay refusal and operator-only authority changes |
| 2 Exclusive catalog | 02, 08: exact live intersection and no ambient MCP fallback |
| 3 No cross-binding credentials; 4 ambient scrub | 03, 04, 09: tenant-bound delivery and synthetic cross-tenant sentinel tests |
| 5 Private CLI config | 04, 05: fresh child HOME/config and owned cleanup |
| 6 Scope freeze | 02, 04: selectors and native override attempts refused before side effects |
| 8 CredentialRefs only; 9 no secrets in MCP | 03, 07, 09: values-free bridge, audit and output conformance |
| 10 Fail closed | All; 06 additionally proves overload refuses without bypass |
| 11 MCP stdout | 07, 09: diagnostics cannot contaminate protocol output |
| 12 Workspace allowlist | 01, 04, 08: expected binding and workspace checked before dispatch/CLI start |

## Shared acceptance and handoff

Every child preserves all twelve AGENTS.md invariants, documents compatibility,
adds positive and refusal tests, and runs format, clippy, workspace typecheck and
touched tests with a temporary HOME/LOCUS_HOME and the original Rust toolchain
paths. Validation commands in the JSON identify existing regression anchors;
children must add the named acceptance scenarios, not merely rerun old tests.

Evidence paths are investigation starting points, not permission to modify a
protected path. Read CODEOWNERS before implementation. Tier-1 changes require a
separately authorized owner task; do not work around restrictions. Do not edit
.github, CODEOWNERS or release/publish configuration under this queue mandate.
Never commit/push/publish, print environment values or resolve real secrets in
fixtures. No dead runtime code was removed in this planning-only change.

A completion handoff includes changed paths, exact commands and counts, negative
case evidence, remaining external blockers, compatibility impact and rollback.
No job is production-ready based solely on metadata readiness, a graph allow
event, a local advisory approval, or installed Phantom binaries.

Validate this backlog locally with:

```bash
cargo test -p locus-core --test swarm_plan
```

The contract tests reject unknown fields, duplicate IDs, missing dependencies,
cycles, invalid priorities, empty acceptance/test contracts, absent evidence,
and drift between this dispatch index and the machine-readable records. They
validate plan integrity, not runtime security or future implementation claims.
