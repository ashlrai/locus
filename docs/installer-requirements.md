# Shell installer requirements

This document separates the request in [issue #169](https://github.com/ashlrai/locus/issues/169)
from implementation choices and decisions still to be made. It is a
requirements record, not evidence that a hosted installer or release assets
are available.

## Stated outcome and constraints

- Provide a one-command shell installation path that downloads and verifies a
  release archive, in the shape of the direnv installer.
- Cover machines with neither npm nor Rust; this path therefore cannot depend
  on either npm or a local Rust build.

The issue motivates this request with first-run measurements: npm required two
commands, while Cargo took longer and failed with an older default toolchain.
Those measurements came from one Linux x86-64 VM and are not performance
requirements or evidence of behavior on other hosts. See
[first-run results](FIRST-RUN.md) and [benchmarks](BENCHMARKS.md).

## Current implementation context

The repository's [installer](../scripts/install.sh) installs both `locus` and
`locus-mcp` from a target-specific archive and checks its SHA-256 sidecar before
extracting the binaries. A checksum checks consistency with that sidecar; it
does not establish publisher authenticity if both files are replaced. These are
implementation choices and constraints, not details specified by the issue.
The script requires Bash, curl, tar, common shell utilities, a checksum tool,
network access and a writable destination; it does not require npm or Rust.

The [platform matrix](installer-platform-matrix.md) records current selections
and prerequisites. Use the [acceptance checklist](installer-acceptance.md) to
evaluate a candidate release, including native execution and failure cases.
Offline tests use fixtures and cannot establish hosted availability or native
binary compatibility.

## Evidence needed for the proposed path

The repository script is an implementation, not by itself a published
one-command installation path. Before describing the issue's goal as delivered,
record the following for a specific release and installer revision:

1. A documented, publicly reachable command and the prerequisites needed to run
   it on machines without npm or Rust.
2. The supported native platforms and matching release archives and verification
   material, with the trust model stated for the published assets.
3. Successful installation and execution on the supported native hosts, plus
   failure checks showing that missing or invalid verification material stops
   installation. Record the release, host and results in the
   [acceptance checklist](installer-acceptance.md).

These are proposed release-readiness checks, not claims that the hosted command,
assets or native-host results already exist. Offline fixture tests help exercise
the script but do not replace the release and host checks.

## Open questions

- Where will the one-command entry point be hosted, and which releases will
  publish the installer, archives and verification material?
- Which operating systems, architectures and Linux libc baselines are supported,
  including Linux ARM, Windows, WSL and musl? Which native hosts will be tested?
- What verification and trust model is required: is a downloaded checksum
  sufficient, or is independently authenticated publisher provenance required?
- What are the intended version selection, installation destination, PATH,
  upgrade and failure/rollback guarantees? Which prerequisites and permissions
  are acceptable for the published command?
- Is installing both binaries required for the published path, and what
  constitutes successful installation and first-run verification?

The issue does not settle these decisions. The current script's behavior is
documented in the [archive selection](shell-installer-archive-selection.md) and
[failure matrix](installer-failure-matrix.md); it should not be mistaken for a
promise that every release or host is supported.
