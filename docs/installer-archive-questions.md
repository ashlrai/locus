# Installer archive questions

[Issue #169](https://github.com/ashlrai/locus/issues/169) requests a
one-command shell installation that downloads and verifies a release archive,
in the shape of the direnv installer, for machines with neither npm nor Rust.
This document collects the unresolved archive selection and installation
questions that request leaves open. It does not claim that a hosted
installer, published release assets, or a supported platform matrix exist,
and it does not assert that the repository's
[installer](../scripts/install.sh) resolves any of these questions; that
script is a candidate whose choices still require decisions and evidence.

The issue motivates the request with first-run measurements from one Linux
x86-64 VM: npm needed two commands, while `cargo install` on a current stable
toolchain took 86.488 s and failed on the machine's older default toolchain.
These observations justify the request; they are not timing targets or a
platform support guarantee. See [first-run results](FIRST-RUN.md) and
[benchmarks](BENCHMARKS.md).

For each answer, record the installer revision, release tag, host or simulated
platform, command, exit status, and observed state. Use disposable hosts and
destinations, and do not record credentials. Mocked platform detection and
downloads do not establish native compatibility or published asset
availability. Questions about the error contract — what each failure reports
and what state it leaves — are tracked separately in the
[error questions](installer-error-questions.md) record and the
[failure matrix](installer-failure-matrix.md).

## Archive selection

- Which operating system and architecture pairs will the published command
  serve, and which reported `uname` values and aliases (such as `arm64` and
  `aarch64`, or `x86_64` and `amd64`) will select each release target?
- Which Linux libc baseline is supported: will musl hosts get their own
  archive, be rejected, or be served another way, and what minimum glibc
  version will be tested?
- How should translation and compatibility layers be handled — should an
  x86-64 shell under Rosetta on Apple Silicon select the Intel archive or
  detect native hardware, and which WSL versions and distributions are
  supported?
- What archive format, filename convention, and internal layout are required,
  and must every archive carry both `locus` and `locus-mcp` as regular
  executable members?
- How is the release selected — `latest` or an explicit version, with and
  without a `v` prefix — and must the archive and its verification material
  come from the same resolved tag?
- Must every intended release publish the archive and verification sidecar
  for every supported target, and should a mapped target whose assets are
  missing from a release fail or select an alternative?
- What verification material and algorithm will the published path require,
  and how is that material authenticated? A checksum fetched beside the
  archive checks consistency with its sidecar, not publisher authenticity if
  both are replaced — is a stronger trust property required, and how is it
  provided?

## Installation

- Where will the one-command entry point be hosted, what is its exact form,
  and how should a user establish trust in it before running it?
- Which prerequisites may the published command assume beyond the excluded
  npm and Rust — shell, download, extraction, and checksum tools plus network
  access — and how should it behave when one is absent?
- What is the default installation directory, how does the user override it,
  and what happens when `HOME` is unset or the destination is relative or
  contains spaces?
- What permissions may the command assume — is a user-writable destination
  required, may system-wide locations be offered, and may it ever elevate?
- Who owns `PATH`: may the installer edit shell profiles or the environment,
  or must it only report how to add the destination?
- What upgrade, downgrade, and rollback behavior is required when installing
  over existing binaries, given that two binary replacements are involved —
  is an atomic pair, an installation record, or an uninstall mode required,
  and how are Locus state, bindings, and client configuration treated on
  removal?
- What constitutes a successful installation and first run — which
  executables must exist, which version output must they report, and how
  does the user confirm them?

## Evidence boundary

The repository's [installer](../scripts/install.sh) makes concrete choices —
`.tar.gz` archives, SHA-256 sidecars, three mapped targets, a `~/.local/bin`
default — but those are implementation behavior, not decisions taken by the
issue, and the script's presence does not establish a hosted entry point or
release assets. Current behavior is recorded in the
[archive selection guide](shell-installer-archive-selection.md) and
[platform matrix](installer-platform-matrix.md); requirements are tracked in
[installer requirements](installer-requirements.md),
[archive requirements](installer-archive-requirements.md), and
[archive installer requirements](archive-installer-requirements.md). Evaluate
candidates against the [acceptance checklist](installer-acceptance.md),
[acceptance cases](installer-acceptance-cases.md), and
[issue checklist](shell-installer-issue-checklist.md); the
[user journey](installer-user-journey.md) and
[failure cases](shell-installer-failure-cases.md) cover destination behavior
and troubleshooting. The offline suite `bash scripts/test-install.sh`
exercises the candidate with fixtures; it cannot settle these questions.
