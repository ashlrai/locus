# Shell installer archive selection

[Issue #169](https://github.com/ashlrai/locus/issues/169) asks for a
one-command installation from a verified release archive on machines without
npm or Rust. The issue does not specify a platform matrix. This document records
what the repository's [installer](../scripts/install.sh) selects today and the
questions that must be resolved before claiming broader support. It does not
establish a published installer URL or asset availability for any release.

## Current selection requirements

The installer reads `uname -s` and `uname -m`, matches the pair exactly, and
selects the following target. Each row lists literal reported values, including
aliases; the resulting filename is `locus-<target>.tar.gz`.

| `uname -s` | `uname -m` | Target |
| --- | --- | --- |
| `Darwin` | `arm64` | `aarch64-apple-darwin` |
| `Darwin` | `aarch64` | `aarch64-apple-darwin` |
| `Darwin` | `x86_64` | `x86_64-apple-darwin` |
| `Darwin` | `amd64` | `x86_64-apple-darwin` |
| `Linux` | `x86_64` | `x86_64-unknown-linux-gnu` |
| `Linux` | `amd64` | `x86_64-unknown-linux-gnu` |

All other pairs fail as unsupported before any release download; the error
suggests a source install, which itself requires a suitable Rust toolchain and
is not guaranteed to work on every host. The script does not accept a target
override or select an alternative archive when an asset is missing.

For the selected target, the archive and its `locus-<target>.tar.gz.sha256`
sidecar must exist under the same resolved release tag. `latest` is resolved
once; an explicit version is normalized to a `v`-prefixed tag. The installer
checks the sidecar against the archive before reading its two required binary
members, `locus` and `locus-mcp`. A matching checksum checks integrity against
that sidecar, not publisher authenticity if both files are replaced. See the
[platform matrix](./installer-platform-matrix.md) for prerequisites and the
[failure matrix](./installer-failure-matrix.md) for failure behavior.

## Questions and evidence still needed

These are open decisions or unverified cases, not promises of support:

- **Linux libc and baseline:** x86-64 selects the GNU archive without detecting
  libc. Should musl hosts have a separate build, be rejected, or be supported
  another way? What minimum glibc version and distribution baseline will be
  supported and tested? A successful selection does not prove execution.
- **Other OS and architectures:** Should Linux ARM64, Windows, or other hosts
  receive release assets and mappings? Which aliases and architectures should
  be accepted, and on which native systems will both binaries be tested?
  Currently they are rejected rather than mapped to a guessed target.
- **Translation and compatibility layers:** On Apple Silicon running an x86-64
  shell under Rosetta, should the reported process architecture select Intel,
  or should native hardware be detected? What WSL versions, distributions and
  libc combinations should be supported? WSL reporting Linux x86-64 currently
  selects the GNU target; neither `uname` nor selection proves compatibility.
- **Release and delivery:** Will every supported target have both assets for
  every intended release, and where will the one-command entry point be hosted?
  Missing assets fail the download; the script's presence and mocked tests do
  not establish a hosted endpoint or release coverage.

For each candidate release, record the release tag, installer revision, host,
command, asset and sidecar availability, and native execution results for both
binaries using the [acceptance checklist](./installer-acceptance.md). The
[offline installer tests](../scripts/test-install.sh) exercise selection with
mocked `uname` and downloads; they do not prove native binary compatibility.
