# Dry-run acceptance checklist

Use this checklist to review changes addressing [issue #171](https://github.com/ashlrai/locus/issues/171).
It describes acceptance for the CLI in this checkout; check each applicable
item and record the commands and evidence in the review. The [command matrix](dry-run-matrix-q8.md)
is the source for command classification and exceptions, not an assertion that
every mutation in Locus has a preview. The [CLI documentation](README.md#previewing-changes-with---dry-run)
and [agent contract](agent-contract.md#--dry-run) define the output and limits.

## Scope and mutation avoidance

- [ ] Classify each changed command form using the matrix: plan, accepted and
  ignored, or unsupported. Check required arguments and flag placement in its
  help. Include command-specific modes such as `autopin suggest --apply` and
  `mcp mint`, rather than assuming all subcommands share their parent's class.
- [ ] For a plan, verify successful and failed previews leave the Locus home,
  binding and session state, workspace and client configuration, and export
  targets unchanged. Compare existence, contents and relevant metadata in an
  isolated fixture, including both missing and initialized homes. Verify no
  child or server starts, no credential is resolved, no provider mutation is
  made, and no control broker is started. Review paths beyond the snapshot as
  well: a filesystem snapshot alone cannot prove absence of transient writes
  or external effects. See the [manual verification procedure](dry-run-verification.md)
  and [README preview contract](../README.md#previewing-mutations-with---dry-run).
- [ ] Preserve the documented exceptions: `setup --print` and `setup --client
  generic` print configuration; accepted-and-ignored commands retain normal
  behavior, including store creation and fail-closed drift freezing. Do not
  suppress a security control to make an ignored flag appear side-effect-free.
- [ ] Confirm unsupported child and server forms do not run under a Locus
  preview. Arguments after `--` belong to the child; provider operations and
  MCP tool calls are outside this flag's scope. Adapter install and registry
  index add plans do not fetch or verify remote content.

## Useful output

- [ ] Check human and JSON output against the actual proposed operation and
  targets, including create versus overwrite, deletion, and an empty plan when
  nothing needs doing where applicable. JSON plans use `locus.plan.v1`,
  `ok: true`, `dry_run: true`, command, changes, control-capability information
  and notes; command-specific legacy fields may also be present. See the
  [plan shape](agent-contract.md#--dry-run).
- [ ] Verify output identifies relevant preconditions and limitations without
  disclosing credentials, capabilities, bearer tokens or other secrets. A
  successful plan is not authorization and does not guarantee that the real
  command will succeed; capability presence is not authentication.

## Error handling

- [ ] Exercise missing or invalid inputs and failed preconditions (for example,
  an unknown binding or blocked workspace). They must report a useful error,
  return nonzero, and leave state unchanged; they must not produce a successful
  plan. Preserve scope and policy enforcement on the later real operation.
- [ ] Verify documented rejection and exit codes: `doctor --fix` and `offboard`
  reject `--apply --dry-run` with exit 2; `agent setup` rejects it with exit 1;
  `pin --rewrite-configs --dry-run` is rejected. `exec --dry-run -- <command>`
  exits 1; other placements and unsupported forms reject the flag with exit 2.
  Check diagnostic and stdout behavior using the [exit-code contract](exit-codes.md)
  and [matrix](dry-run-matrix-q8.md#unsupported-cases).

## Evidence for review

- [ ] Run the [manual verification procedure](dry-run-verification.md) and
  focused tests for affected command forms, including human/JSON output,
  unchanged state and failure cases. Extend coverage for changed behavior.
- [ ] Run the workspace tests, formatting and lint checks. Record failures and
  any coverage limitations; do not skip or delete tests to satisfy this review.
