# Archive installer design questions

[Issue #169](https://github.com/ashlrai/locus/issues/169) requests a
one-command shell installer that downloads and verifies a release archive on
machines without npm or Rust. The issue does not decide the contracts below.
These are open questions, not claims that a hosted installer, release assets,
or any proposed behavior is implemented or supported.

## Platform detection

- Which operating systems and architectures should be supported, and how
  should their reported names be normalized into release targets?
- How should the installer detect Linux libc and its minimum compatible
  version, and distinguish native machines from environments such as Rosetta
  or WSL? Which combinations need native-host verification?
- What should happen when detection is ambiguous or a platform is unsupported,
  and which alternative installation instructions should be offered?

## Archive selection

- How should a requested version or the latest release resolve to an exact
  tag, archive name, and target? What happens if an asset is absent for that
  target or release?
- What archive format and layout should be required, including the names and
  types of both binaries? How should unsafe paths, links, duplicates, or missing
  members be handled?
- What verification material should accompany each archive, and how will its
  authenticity be established? A checksum fetched alongside an archive can
  detect corruption but cannot authenticate the publisher if both are replaced.

## Installation location

- What should the default destination be, how should users override it, and
  how should unset home directories and relative paths be handled?
- Should installation require a user-writable directory or offer a system-wide
  location? How should the command handle an existing installation and a
  destination absent from PATH, including changes to shell profiles?
- What upgrade, downgrade, uninstall, and rollback guarantees are required,
  especially when replacing two binaries or when an installation is interrupted?

## Failure behavior

- What exit statuses and actionable diagnostics should distinguish unsupported
  platforms, missing prerequisites or assets, network failures, verification
  failures, malformed archives, and destination errors?
- At which stages must existing binaries remain unchanged, and what cleanup or
  recovery is required if replacement fails partway through? Should retries be
  bounded, and how should interruptions be handled?
- Which failure cases can be exercised with synthetic fixtures, and which need
  real release assets or native hosts before support can be claimed?

Record decisions alongside their evidence and acceptance criteria. For current
candidate behavior and further detail, see the [platform matrix](installer-platform-matrix.md),
[archive requirements](installer-archive-requirements.md), [error questions](installer-error-questions.md),
and [acceptance checklist](archive-installer-acceptance.md). Mocked tests do not
establish native compatibility or availability of a published installer.
