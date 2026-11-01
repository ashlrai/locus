# Archive installer requirements

This is a requirements record for [issue #169](https://github.com/ashlrai/locus/issues/169).
It restates the requested installation path, the prerequisites the issue
excludes, and the decisions the issue leaves open. It does not specify
published assets or prove that a hosted one-command installer exists.

## Goal from the issue

Provide a **one-command archive installation**: a single shell invocation,
in the shape of the direnv installer, that downloads a release archive,
verifies it, and installs the Locus binaries. No checkout, package-manager
install step, or source build stands between the user and a working `locus`
and `locus-mcp`.

The issue motivates the request with first-run measurements from one Linux
x86-64 VM: the npm path needed two commands, while `cargo install` on a
current stable toolchain took 86.488 s and failed on the machine's older
default toolchain. These observations justify the request; they are not
timing targets or a platform support guarantee. See
[first-run results](FIRST-RUN.md) and [benchmarks](BENCHMARKS.md).

## Prerequisite exclusions

The issue targets machines with **neither npm nor Rust**, so for this path:

- npm and Node.js cannot be prerequisites. The npm wrapper is an alternative
  installation route, not something this command may require or fall back to.
- A Rust toolchain cannot be a prerequisite. The command must install
  prebuilt release binaries and must never compile from source; a source
  install is a separate path with its own toolchain requirements.

The path may still carry non-npm, non-Rust prerequisites. The repository's
[installer](../scripts/install.sh) requires Bash, `curl`, `tar`, `uname`,
`mktemp`, `mkdir`, `chmod`, `mv`, `rm`, `tr`, and `sha256sum` or `shasum`,
plus HTTPS access to GitHub releases and a writable destination. Which
prerequisites the published command may assume — and how it behaves when one
is missing — remain open; see the decisions below and the
[platform matrix](installer-platform-matrix.md).

## Unresolved decisions

The issue states the goal and the exclusions but does not settle the
following. Each needs a decision and supporting evidence before the
one-command path can be called supported.

### Platform coverage

- Which operating systems and architectures will the published command
  serve, and which will be rejected with a pointer to alternatives? The
  repository's installer maps only macOS arm64/x86-64 and Linux x86-64;
  Linux ARM, Windows, and other pairs are refused.
- Which Linux libc baselines are supported? The current script selects the
  GNU archive without detecting libc; whether musl hosts get their own
  archive and what minimum glibc version will be tested is undecided.
- How are translation and compatibility layers treated — Rosetta on Apple
  Silicon, WSL distributions reporting Linux x86-64 — and on which native
  hosts will both installed binaries be executed for each supported target?

The [archive selection guide](shell-installer-archive-selection.md) and
[platform matrix](installer-platform-matrix.md) record today's mappings and
their limits. Mocked platform detection in the offline tests does not prove
native compatibility.

### Installation location

- Where will the binaries be installed by default, and how does the user
  choose another destination? The repository's installer defaults to
  `~/.local/bin`, requires `--bin-dir` when `HOME` is unset, and treats
  relative destinations as relative to the working directory; the published
  default and override contract are undecided.
- What permissions may the command assume? The current script needs a
  user-writable destination and never uses sudo; whether the published path
  may offer system-wide locations, and under what conditions, is open.
- Who owns `PATH`? The current script prints a reminder when the
  destination is absent from `PATH` and never edits shell profiles; whether
  the published command may modify profile files or the environment is
  undecided.
- What upgrade, downgrade, and rollback behavior is required at the chosen
  location, including interruption between the two binary replacements? Is
  an uninstall mode or installation record required, and how are Locus
  state, bindings, and client configuration treated on removal?

The [user journey](installer-user-journey.md) records today's destination
behavior and uninstall questions; the [failure matrix](installer-failure-matrix.md)
and [error questions](installer-error-questions.md) record failure behavior
and the error contract still to be decided.

## Evidence boundary

The repository's [installer](../scripts/install.sh) makes concrete choices
for the questions above; those choices are implementation behavior, not
decisions taken by the issue, and the script's presence does not establish a
hosted entry point or release assets. This record complements the
[archive requirements](installer-archive-requirements.md): that document
covers what the command downloads and how it is verified, while this one
covers the command itself and where its results land. Use the
[acceptance checklist](installer-acceptance.md) to evaluate a candidate
release on a real host.
