# Shell installer issue checklist

[Issue #169](https://github.com/ashlrai/locus/issues/169) proposes a
one-command shell installation that downloads and verifies a release archive,
including for machines without npm or Rust. This is a checklist for assessing
that goal, not a claim that a published installation command or release assets
are available. Leave items unchecked until there is evidence for the candidate
release and host.

## Goal and release readiness

- [ ] Provide and test one published command that downloads and verifies the
  appropriate release archive and installs `locus` and `locus-mcp`.
- [ ] Run that command on a clean supported machine without npm, Node.js,
  Cargo, or Rust; it must not require a source build. Document the remaining
  shell utilities, network access, and destination permissions it needs.
- [ ] Confirm the archive and checksum sidecar are available for the candidate
  release and target. A checksum verifies consistency with the sidecar, not
  publisher authenticity if both are replaced.
- [ ] Run both installed binaries and record the release tag, installer
  revision, platform, command, and result. Repeat for each supported target;
  mocked downloads and platform detection do not prove native compatibility.
- [ ] Check failure behavior, including missing assets and checksum mismatch,
  before relying on the installer for an existing installation.

The repository contains [`scripts/install.sh`](../scripts/install.sh); its
presence alone does not establish a hosted entry point or asset availability.
Use the [acceptance checklist](installer-acceptance.md) for detailed installation,
upgrade, and failure checks, and the [platform matrix](installer-platform-matrix.md)
for supported target selection and unresolved compatibility questions.
