# Locus documentation

Agents start here. Locus is the identity plane for coding agents: a person pins one client, and the CLI, the MCP tools, and the local dashboard act only as that client until a person pins again. Every command block in [copy-paste.md](copy-paste.md) is meant to be pasted.

| Read this | For |
| --- | --- |
| [copy-paste.md](copy-paste.md) | Install, JSON, MCP, the first pin |
| [agent-contract.md](agent-contract.md) | `--json` schemas, exit codes, dry-run |
| [mcp.md](mcp.md) | Claude Code, Cursor, Codex, supervised launch |
| [../llms.txt](../llms.txt) | Short index for agents |
| [../llms-full.txt](../llms-full.txt) | Heading index of the README and these docs |
| [../AGENTS.md](../AGENTS.md) | Rules when changing this repo |
| [../CONTRIBUTING.md](../CONTRIBUTING.md#contribute-with-an-agent) | Contribute with an agent: fork, worktree, `fast-check.sh`, CI Gate |
| [architecture.md](architecture.md) | How a pin becomes the only account |
| [credential-compatibility.md](credential-compatibility.md) | `env:` and `phm:` refs |
| [deepwiki.md](deepwiki.md) | DeepWiki auto-docs config (`.devin/wiki.json`) |

## Where Locus fits

Locus is one of the open-source [Ashlr](https://ashlr.ai) tools: Phantom, phantom-secrets, ashlr-plugin, Locus, Lexicon, ashlr-md, and ashlr-stack. Together they aim to be a fully autonomous, code-first, agent-native software factory: agents do the work, and the human acts as a passive board member. The stated goal is chasing open-source RSI (recursive self-improvement). That is a direction, not a finished capability. In that factory Locus is the identity gate: a person pins, and agents act only inside the pin. Agents cannot pin; they can only ask for one. Each tool installs and runs on its own. See [README: Part of the Ashlr ecosystem](../README.md#part-of-the-ashlr-ecosystem).

Agents also change this repo. They follow [../AGENTS.md](../AGENTS.md) and [../CONTRIBUTING.md](../CONTRIBUTING.md), and every change lands as a pull request that passes the required **CI Gate** check ([ci.yml](../.github/workflows/ci.yml)) and then goes through the merge queue. To send a change with a coding agent, start at [Contribute with an agent](../CONTRIBUTING.md#contribute-with-an-agent).

## All docs by task

Every page in this folder appears once below.

### Start and daily use

- [copy-paste.md](copy-paste.md): paste-ready blocks for install, JSON, MCP and the first pin
- [agent-contract.md](agent-contract.md): `--json` schema ids, exit codes, `--dry-run` and non-interactive rules
- [FIRST-RUN.md](FIRST-RUN.md): what the README install and first commands did on a clean home (2026-10-26)
- [onboarding.md](onboarding.md): `locus onboard`, three clients across three tenants
- [broker-recovery-q8.md](broker-recovery-q8.md): recovering a pin after its broker exits
- [doctor-fix.md](doctor-fix.md): `locus doctor --fix` repair previews
- [exit-codes.md](exit-codes.md): stable CLI exit codes
- [shell-completions.md](shell-completions.md): `locus completion` install
- [cli-routing.md](cli-routing.md): `locus exec --for` with `gh`, `vercel`, `supabase`, `aws`
- [autopin-suggest-output.md](autopin-suggest-output.md): `locus autopin suggest` output and examples
- [build-q8.md](build-q8.md): building the `locus` and `locus-mcp` executables
- [MSRV.md](MSRV.md): minimum supported Rust version

### Architecture, policy, and security

- [architecture.md](architecture.md): planes and how a pin becomes the only account
- [policy.md](policy.md): policy rules (allow, deny, `require_approval`) and `locus approve`
- [credential-compatibility.md](credential-compatibility.md): `env:` and `phm:` refs
- [credential-audit.md](credential-audit.md): signed credential injection audit (opt-in)
- [session-budget.md](session-budget.md): tool-call and time caps per session (opt-in)
- [gateway-base-url.md](gateway-base-url.md): passing a model gateway base URL to `locus exec` (opt-in)
- [gateway-base-urls.md](gateway-base-urls.md): default gateway and base URL behavior (audit note)
- [verification-plane.md](verification-plane.md): `locus verify claim` and `locus verify session`
- [agency-certainty.md](agency-certainty.md): identity versus epistemic layers (design note)
- [PQ-READINESS.md](PQ-READINESS.md): post-quantum readiness status

### Agencies and multi-client work

- [firm-mode.md](firm-mode.md): multi-client and dual-control operations
- [agency-starter.md](agency-starter.md): agency starter kit and the doctor single pane
- [dogfood-multi-account.md](dogfood-multi-account.md): personal plus client dogfood walkthrough
- [observability.md](observability.md): `locus events export`, fleet pulse, SIEM NDJSON, OTLP

### MCP, swarms, and workers

- [mcp.md](mcp.md): client wiring and the supervised server
- [swarm-broker.md](swarm-broker.md): grant-scoped swarm broker (`locus serve --swarm`)
- [swarm-child-jobs.md](swarm-child-jobs.md): swarm broker child jobs
- [peers.md](peers.md): local agent peers and message relay
- [workers.md](workers.md): worker backends
- [exec-latency.md](exec-latency.md): `locus exec` latency and launcher comparison
- [BENCHMARKS.md](BENCHMARKS.md): measured install and latency numbers, with method

### Phantom and the Ashlr ecosystem

- [hub-integration.md](hub-integration.md): Phantom and Locus integration contract
- [PHANTOM-USAGE.md](PHANTOM-USAGE.md): using Locus with Phantom
- [phantom-enrolled-repos.md](phantom-enrolled-repos.md): Locus in Phantom-enrolled repos
- [phantom-preflight.md](phantom-preflight.md): the Phantom preflight in `locus doctor`
- [ashlr-handshake.md](ashlr-handshake.md): Ashlr companion handshake (v1)

### Adapters and the adapter registry

- [adapters.md](adapters.md): writing a provider adapter
- [adapter-sdk.md](adapter-sdk.md): adapter SDK and manifest
- [registry-trust.md](registry-trust.md): adapter registry release manifests
- [registry-manifest-verification.md](registry-manifest-verification.md): verifying a release manifest
- [registry-manifest-consumer.md](registry-manifest-consumer.md) and its [checklist](registry-manifest-consumer-checklist.md): consuming a release manifest
- [registry-unsigned-consumer-guidance.md](registry-unsigned-consumer-guidance.md) and [registry-unsigned-manifest-faq.md](registry-unsigned-manifest-faq.md): unsigned manifests
- [registry-manifest-current-state.md](registry-manifest-current-state.md), [registry-manifest-provenance.md](registry-manifest-provenance.md), [registry-manifest-ci-role.md](registry-manifest-ci-role.md): current state, provenance, and CI's role
- [registry-signing-status.md](registry-signing-status.md), [registry-trust-ci-status.md](registry-trust-ci-status.md), [registry-trust-actors.md](registry-trust-actors.md), [registry-trust-faq.md](registry-trust-faq.md): signing and trust status
- [registry-signing-open-questions.md](registry-signing-open-questions.md), [registry-manifest-verification-questions.md](registry-manifest-verification-questions.md): open questions

### Dry-run (`--dry-run`)

The overview is [below](#previewing-changes-with---dry-run). Details: [dry-run matrix](dry-run-matrix-q8.md), [manual verification](dry-run-verification.md), [acceptance checklist](dry-run-acceptance.md), [help review matrix](dry-run-review-matrix.md).

### Contributing and releases

- [autopin-review-checklist.md](autopin-review-checklist.md): review checklist for autopin changes
- [conflict-drivers.md](conflict-drivers.md): deterministic merge resolution for high-churn paths
- [deepwiki.md](deepwiki.md): DeepWiki auto-docs config
- [RELEASE.md](RELEASE.md): releasing Locus
- [release-automation.md](release-automation.md): release automation

### Proposed one-command installer (design notes)

Not shipped. Current install routes are in the [README](../README.md#install-options).

- Journey and requirements: [user journey](installer-user-journey.md), [requirements](installer-requirements.md), [prerequisites](installer-prerequisites.md), [issue checklist](shell-installer-issue-checklist.md)
- Archives: [archive installer requirements](archive-installer-requirements.md), [installer archive requirements](installer-archive-requirements.md), [archive selection](shell-installer-archive-selection.md), [installer archive questions](installer-archive-questions.md), [archive installer design questions](archive-installer-design-questions.md)
- Platforms and errors: [platform matrix](installer-platform-matrix.md), [platform questions](installer-platform-questions.md), [failure matrix](installer-failure-matrix.md), [failure cases](shell-installer-failure-cases.md), [error questions](installer-error-questions.md)
- Acceptance: [acceptance checklist](installer-acceptance.md), [acceptance cases](installer-acceptance-cases.md), [archive installer acceptance](archive-installer-acceptance.md), [acceptance evidence](installer-archive-acceptance.md)

### Test harness internals

For people changing the dogfood, E2E, and Hub smoke scripts.

- Dogfood: [temporary resources](dogfood-temporary-resources.md), [temp prefixes](dogfood-temp-prefix-reference.md), [temp failures](dogfood-temp-failures.md), [probe cleanup](dogfood-probe-cleanup.md), [probe isolation](dogfood-probe-isolation.md), [probe roles](dogfood-probe-roles.md)
- E2E: [test map](e2e-test-map.md), [temp resource types](e2e-temp-resource-types.md), [graph isolation](e2e-graph-isolation.md), [graph lifecycle](e2e-graph-lifecycle.md), [security directories](e2e-security-directories.md), [security isolation](e2e-security-isolation.md)
- Hub integration test: [isolation](hub-integration-isolation.md), [temp lifecycle](hub-integration-temp-lifecycle.md), [temp paths](hub-integration-tmpdir.md), [temp parent and prefixes](temp-parent-reference.md), [Hub test comparison](hub-test-temp-comparison.md)
- Hub smoke: [prerequisites](hub-smoke-prerequisites.md), [output](hub-smoke-output.md), [interruption](hub-smoke-interruption.md), [cleanup review](hub-smoke-cleanup-review.md), [resources](hub-smoke-resources.md), [temp resources](hub-smoke-temp-resources.md), [resource map](hub-smoke-resource-map.md), [resource roles](hub-smoke-resource-roles.md), [temp lifecycle](hub-smoke-temp-lifecycle.md), [temp failures](hub-smoke-temp-failures.md)

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
