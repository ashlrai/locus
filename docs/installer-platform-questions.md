# Shell installer platform support questions

[Issue #169](https://github.com/ashlrai/locus/issues/169) requests a verified
release-archive installer for machines without npm or Rust; it does not define
which operating systems or architectures must be supported. These questions
are unresolved decisions, not a promise that any candidate platform works.

## Current evidence

The repository's [installer](../scripts/install.sh) selects archives for macOS
ARM64, macOS x86-64 and Linux x86-64 GNU based on `uname`; other OS and
architecture pairs are rejected before download. Linux selection does not
detect libc. The [platform matrix](installer-platform-matrix.md) records the
exact aliases and prerequisites. Selection is not evidence that a particular
release has the assets or that its binaries run on a given host. The
[offline tests](../scripts/test-install.sh) use mocked detection and downloads.

## Decisions needed

- **Operating systems:** Which macOS versions and Linux distributions should
  be covered? Should Windows have a shell installer path, and should other
  operating systems be considered? What prerequisites are acceptable on each?
- **Architectures:** Which combinations should have archives and installer
  mappings, including Linux ARM64, 32-bit systems, and any other architectures?
  Which `uname` aliases are valid for each, and how should unknown values fail?
- **Linux ABI:** Should musl systems receive a separate archive or a clear
  rejection? What minimum glibc version and distribution baseline should the
  GNU build target? How should libc be detected without guessing compatibility?
- **Compatibility layers:** Should WSL be covered, and for which versions,
  distributions and libc combinations? On Apple Silicon with an x86-64 shell
  under Rosetta, should selection follow the reported process architecture or
  the hardware? How will those choices be tested?
- **Release coverage:** For which releases must every chosen target have both
  an archive and its checksum sidecar? How will missing assets be reported and
  how will the hosted one-command entry point be validated?

## Evidence before a support claim

Agree on each OS, version, architecture and ABI combination and record the
candidate release tag, installer revision, host details and commands. Check the
archive and sidecar for that tag, install on a representative real host, and run
both binaries. Record failures as well as successes; exercise rejection paths
for excluded combinations. Use the [acceptance checklist](installer-acceptance.md)
for the full procedure. Mocked selection tests alone cannot establish native
execution, release availability or a hosted entry point.
