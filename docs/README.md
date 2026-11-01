# Locus documentation

For autopin implementation changes, use the [autopin review checklist](autopin-review-checklist.md).
For the suggestion command's output and examples, see [autopin suggest output](autopin-suggest-output.md).
For the proposed one-command archive installer, see the [user journey](installer-user-journey.md), [requirements](installer-requirements.md), and [issue checklist](shell-installer-issue-checklist.md).
For archive requirements and unresolved release decisions, see [installer archive requirements](installer-archive-requirements.md).
For testable archive installation outcomes, see [installer acceptance cases](installer-acceptance-cases.md).
For open error-contract decisions, see [installer error questions](installer-error-questions.md).
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
