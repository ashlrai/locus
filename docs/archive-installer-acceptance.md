# Archive installer acceptance

[Issue #169](https://github.com/ashlrai/locus/issues/169) proposes a one-command
shell installation that downloads and verifies a release archive on machines
without npm or Rust. This document defines outcomes for accepting a candidate;
it does not claim that a hosted installer or release assets are available. The
issue does not specify a platform matrix, archive layout, or trust mechanism.

## Delivery assumptions to validate

The following are assumptions for evaluating the repository's candidate
[`scripts/install.sh`](../scripts/install.sh), not guarantees made by the issue:

- A published, reachable one-command entry point and a candidate release must be
  identified and tested. A script in the repository does not establish either.
- The candidate uses Bash, curl, tar, standard utilities, and `sha256sum` or
  `shasum`; it must not require npm, Node.js, Cargo, Rust, sudo, credentials, or
  an interactive prompt. The archive and matching SHA-256 sidecar must both be
  available for the same release tag and selected target.
- The current script selects `aarch64-apple-darwin`, `x86_64-apple-darwin`,
  and `x86_64-unknown-linux-gnu`. Selection is not proof of published assets
  or native compatibility. Linux musl, Linux ARM, Windows, minimum glibc, and
  other environments remain unverified; see the [platform matrix](installer-platform-matrix.md).
- A checksum establishes consistency with its sidecar, not publisher
  authenticity if both are replaced. The release process must decide and
  document how verification material is authenticated before claiming that
  stronger property.

## Testable outcomes

Use disposable hosts and destinations. For every candidate, record the installer
revision, exact entry command, release tag, OS, architecture, libc where relevant,
exit status, and observed result. Do not record credentials. Leave unrun checks
unverified; a failure in any required check prevents acceptance.

- [ ] Run the actual published one-command entry point on each accepted native
  target without npm or Rust. It exits successfully without a prompt or source
  build, installs executable `locus` and `locus-mcp`, and both binaries report
  the selected release version.
- [ ] Check latest and an explicit version, with and without a `v` prefix. The
  archive and sidecar come from the same resolved tag. An unavailable asset or
  failed version resolution produces a nonzero exit and actionable error.
- [ ] Install to the default `~/.local/bin` and a writable custom `--bin-dir`
  containing spaces. Repeat over existing binaries; both installed files match
  the selected archive, temporary files are cleaned up, and shell profiles and
  existing credentials remain unchanged. When the destination is absent from
  PATH, guidance explains how to add it.
- [ ] On an unsupported OS or architecture and with each required tool missing,
  installation fails clearly before download or replacement. Exercise both
  checksum-tool alternatives independently.
- [ ] Using controlled fixtures, reject missing, malformed, wrong-name,
  duplicate, or mismatched sidecars before reading the archive. Reject corrupt
  archives and missing, empty, linked, directory, or duplicate binary members.
  In each pre-replacement failure, both seeded destination files remain unchanged.
- [ ] Reject unsafe destinations and report write or rename failures. Check
  cleanup and recovery; the two final replacements are separate, so a failure
  during the second rename can leave mixed versions and does not promise rollback.

## Evidence and limits

Run `bash scripts/test-install.sh` for offline regression coverage with mocked
downloads and platform detection. Fixtures can test failure behavior, but cannot
prove a hosted command, release availability, or native binary compatibility.
Record actual asset availability and native execution separately for each
accepted target and release. The [detailed cases](installer-acceptance-cases.md)
and [failure matrix](installer-failure-matrix.md) provide fixture scenarios and
known limits; the [archive requirements](installer-archive-requirements.md)
track delivery decisions still to be made.
