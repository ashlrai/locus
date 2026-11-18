# Locus documentation

Agents start here. Every command block in [copy-paste.md](copy-paste.md) is meant to be pasted.

| Read this | For |
| --- | --- |
| [copy-paste.md](copy-paste.md) | Install, JSON, MCP, the first pin |
| [agent-contract.md](agent-contract.md) | `--json` schemas, exit codes, dry-run |
| [mcp.md](mcp.md) | Claude Code, Cursor, Codex, supervised launch |
| [../llms.txt](../llms.txt) | Short index for agents |
| [../llms-full.txt](../llms-full.txt) | Heading index of the README and these docs |
| [../AGENTS.md](../AGENTS.md) | Rules when changing this repo |
| [architecture.md](architecture.md) | How a pin becomes the only account |
| [credential-compatibility.md](credential-compatibility.md) | `env:` and `phm:` refs |
| [deepwiki.md](deepwiki.md) | DeepWiki auto-docs config (`.devin/wiki.json`) |

## Reference by topic

Every page in this directory is linked from this index; a test
(`docs_index.rs`) fails when one is not.

| Topic | Pages |
| --- | --- |
| Setup and first run | [FIRST-RUN.md](FIRST-RUN.md), [onboarding.md](onboarding.md), [build-q8.md](build-q8.md), [MSRV.md](MSRV.md), [shell-completions.md](shell-completions.md), [broker-recovery-q8.md](broker-recovery-q8.md) |
| Policy, firm mode, budgets | [policy.md](policy.md), [firm-mode.md](firm-mode.md), [agency-starter.md](agency-starter.md), [agency-certainty.md](agency-certainty.md), [session-budget.md](session-budget.md), [exit-codes.md](exit-codes.md) |
| Workers, routing, peers | [workers.md](workers.md), [cli-routing.md](cli-routing.md), [peers.md](peers.md), [swarm-broker.md](swarm-broker.md), [swarm-child-jobs.md](swarm-child-jobs.md), [conflict-drivers.md](conflict-drivers.md), [verification-plane.md](verification-plane.md) |
| Credentials and gateways | [credential-audit.md](credential-audit.md), [gateway-base-url.md](gateway-base-url.md), [gateway-base-urls.md](gateway-base-urls.md), [PQ-READINESS.md](PQ-READINESS.md) |
| Phantom and Ashlr | [PHANTOM-USAGE.md](PHANTOM-USAGE.md), [phantom-enrolled-repos.md](phantom-enrolled-repos.md), [phantom-preflight.md](phantom-preflight.md), [hub-integration.md](hub-integration.md), [ashlr-handshake.md](ashlr-handshake.md) |
| Adapters | [adapters.md](adapters.md), [adapter-sdk.md](adapter-sdk.md) |
| Adapter registry and signing | [registry-trust.md](registry-trust.md), [registry-trust-actors.md](registry-trust-actors.md), [registry-trust-ci-status.md](registry-trust-ci-status.md), [registry-trust-faq.md](registry-trust-faq.md), [registry-signing-status.md](registry-signing-status.md), [registry-signing-open-questions.md](registry-signing-open-questions.md), [registry-manifest-current-state.md](registry-manifest-current-state.md), [registry-manifest-ci-role.md](registry-manifest-ci-role.md), [registry-manifest-provenance.md](registry-manifest-provenance.md), [registry-manifest-verification.md](registry-manifest-verification.md), [registry-manifest-verification-questions.md](registry-manifest-verification-questions.md), [registry-manifest-consumer.md](registry-manifest-consumer.md), [registry-manifest-consumer-checklist.md](registry-manifest-consumer-checklist.md), [registry-unsigned-consumer-guidance.md](registry-unsigned-consumer-guidance.md), [registry-unsigned-manifest-faq.md](registry-unsigned-manifest-faq.md) |
| Releasing and operations | [RELEASE.md](RELEASE.md), [release-automation.md](release-automation.md), [observability.md](observability.md), [BENCHMARKS.md](BENCHMARKS.md), [exec-latency.md](exec-latency.md) |
| Dogfood probes | [dogfood-multi-account.md](dogfood-multi-account.md), [dogfood-probe-cleanup.md](dogfood-probe-cleanup.md), [dogfood-probe-isolation.md](dogfood-probe-isolation.md), [dogfood-probe-roles.md](dogfood-probe-roles.md), [dogfood-temp-failures.md](dogfood-temp-failures.md), [dogfood-temp-prefix-reference.md](dogfood-temp-prefix-reference.md) |
| End-to-end test notes | [e2e-test-map.md](e2e-test-map.md), [e2e-graph-isolation.md](e2e-graph-isolation.md), [e2e-graph-lifecycle.md](e2e-graph-lifecycle.md), [e2e-security-directories.md](e2e-security-directories.md), [e2e-security-isolation.md](e2e-security-isolation.md), [e2e-temp-resource-types.md](e2e-temp-resource-types.md) |
| Hub smoke notes | [hub-integration-isolation.md](hub-integration-isolation.md), [hub-integration-temp-lifecycle.md](hub-integration-temp-lifecycle.md), [hub-integration-tmpdir.md](hub-integration-tmpdir.md), [hub-smoke-cleanup-review.md](hub-smoke-cleanup-review.md), [hub-smoke-interruption.md](hub-smoke-interruption.md), [hub-smoke-output.md](hub-smoke-output.md), [hub-smoke-prerequisites.md](hub-smoke-prerequisites.md), [hub-smoke-resource-map.md](hub-smoke-resource-map.md), [hub-smoke-resource-roles.md](hub-smoke-resource-roles.md), [hub-smoke-resources.md](hub-smoke-resources.md), [hub-smoke-temp-failures.md](hub-smoke-temp-failures.md), [hub-smoke-temp-lifecycle.md](hub-smoke-temp-lifecycle.md), [hub-smoke-temp-resources.md](hub-smoke-temp-resources.md), [hub-test-temp-comparison.md](hub-test-temp-comparison.md), [temp-parent-reference.md](temp-parent-reference.md) |
| Shell installer | [installer-acceptance.md](installer-acceptance.md), [installer-failure-matrix.md](installer-failure-matrix.md), [installer-platform-matrix.md](installer-platform-matrix.md), [shell-installer-archive-selection.md](shell-installer-archive-selection.md) |

For autopin implementation changes, use the [autopin review checklist](autopin-review-checklist.md).
For the suggestion command's output and examples, see [autopin suggest output](autopin-suggest-output.md).
For the proposed one-command archive installer, see the [user journey](installer-user-journey.md), [requirements](installer-requirements.md), and [issue checklist](shell-installer-issue-checklist.md).
For the remaining archive selection and installation questions, see [installer archive questions](installer-archive-questions.md).
For archive requirements and unresolved release decisions, see [installer archive requirements](installer-archive-requirements.md).
For open platform, archive, location, and failure decisions, see [archive installer design questions](archive-installer-design-questions.md).
For the one-command goal, its prerequisite exclusions, and unresolved platform and installation-location decisions, see [archive installer requirements](archive-installer-requirements.md).
For testable archive installation outcomes, see [installer acceptance cases](installer-acceptance-cases.md).
For release acceptance and delivery assumptions, see [archive installer acceptance](archive-installer-acceptance.md).
For the evidence to record when accepting an archive installer candidate, see [archive installer acceptance evidence](installer-archive-acceptance.md).
For the installer's prerequisites and the questions the issue leaves open, see [installer prerequisites](installer-prerequisites.md).
For open OS and architecture decisions, see the [installer platform questions](installer-platform-questions.md).
For open error-contract decisions, see [installer error questions](installer-error-questions.md).
For the temporary directories and files created by the dogfood script and the Hub smoke helper, and what cleanup does and does not cover, see [dogfood temporary resources](dogfood-temporary-resources.md).
For troubleshooting, see [shell installer failure cases](shell-installer-failure-cases.md).

## Previewing changes with `--dry-run`

The CLI in this checkout accepts the global `--dry-run` flag before or after
supported commands. Supply the command's usual required arguments; consult
`locus <command> --help` for its options. For example:

```sh
locus init --dry-run --json
locus pin personal --dry-run
```

The first command previews initialization without creating the Locus home. The
second needs an initialized home and an existing `personal` binding. A plan
performs read-only preflight checks and writes no state. With `--json`, it emits
an object with schema `locus.plan.v1`; some commands also retain legacy fields.

The following mutating command forms support plans. Names are command forms,
not complete invocations; some require additional arguments.

| Area | Command forms |
| --- | --- |
| Setup and sessions | `init`, `quickstart`, `onboard`, `setup`, `workspace`, `enter`, `switch`, `pin`, `leave`, `renew` |
| Bindings and workspaces | `engagement init`, `engagement close`, `graph export`, `graph import`, `binding add`, `binding rm`, `binding migrate-credential-refs`, `client add`, `offboard`, `autopin suggest --apply` |
| Capabilities and grants | `capability persist`, `capability unpersist`, `ci mint`, `ci env`, `mcp mint`, `mcp revoke` |
| Setup and adapters | `agent setup`, `doctor --fix`, `adapter install`, `adapter uninstall`, `adapter trust add`, `adapter registry export`, `adapter registry index add`, `adapter registry index remove` |
| Approvals and audit | `approve grant`, `approve deny`, `notify on`, `notify off`, `events export`, `forensics export` |

`doctor --fix` and `offboard <alias>` already preview without the flag;
`--apply` performs their changes. Similarly, `binding migrate-credential-refs`
previews by default and requires `--write` to persist. The global flag produces
the plan schema even when a command has its own default preview. `setup --print` and `setup
--client generic` are print-only forms: even with `--dry-run`, they print the
configuration instead of a plan.

### Limits and unsupported operations

- A successful plan is not authorization or a guarantee of success. It does
  not authenticate the control capability or start its authority broker. It
  does not resolve credentials; the real command may still fail its control
  gate or other checks. Adapter install and registry index add plans do not
  fetch or verify remote content.
- Most plans require an initialized Locus home. `init`, `quickstart`, `binding
  add`, `client add`, `agent setup`, and `doctor --fix` can preview against an
  uninitialized home, subject to their options and inputs. Other preconditions, such as binding existence or the workspace
  allowlist, can still cause a preview to fail.
- `doctor --fix --apply --dry-run` and `offboard <alias> --apply --dry-run` are
  rejected (exit 2), as is `agent setup
  --apply --dry-run` (exit 1). Omit `--apply` for these previews. `pin
  --rewrite-configs --dry-run` is also rejected.
- `exec`, `run`, `ci run`, the `mcp` server (without a subcommand), `serve`,
  and `dashboard` cannot be previewed: they run a child or server rather than
  produce a plan. `locus exec --dry-run -- <command>` fails its argument check
  (exit 1); other placements and the other listed commands reject the flag as
  a usage error (exit 2). An argument after `--` belongs to the child, so it
  does not preview the Locus command.
- Read-only commands accept and ignore `--dry-run`; it does not suppress their
  normal behavior. Opening a store can create the Locus home, and session
  inspection can freeze a drifted pin as a fail-closed security measure. See the
  [full command matrix](dry-run-matrix-q8.md) for the read-only forms.
- Other mutating operations do not have a Locus dry-run option. In particular,
  the child commands launched by `exec`, `run`, or `ci run`, provider operations,
  and MCP tool calls are not previewed by this flag. Any dry-run facility for
  those operations must be provided by the child or provider itself.

See the [manual verification procedure](dry-run-verification.md) to check plans
and unchanged state, the [agent contract](agent-contract.md#--dry-run) for plan
details and exit codes, the [acceptance checklist](dry-run-acceptance.md) for
review criteria, the [help review matrix](dry-run-review-matrix.md) for which
command forms document the flag, and [doctor repairs](doctor-fix.md) for its
default preview.
