# Installer error questions

[Issue #169](https://github.com/ashlrai/locus/issues/169) asks for a
one-command shell installer that downloads and verifies a release archive on
machines without npm or Rust. It does not specify the error contract. The
questions below are for deciding and testing that contract; they do not assert
current behavior or the availability of a hosted installer or release assets.
For implementation-specific evidence, see the [failure matrix](installer-failure-matrix.md)
and [failure cases](shell-installer-failure-cases.md).

For each answer, record the installer revision, release, host or simulated
platform, setup, exit status, stdout and stderr, and before/after destination
state. Use disposable directories and synthetic archives for fault injection;
keep real credentials and private paths out of shared records. Compare both a
fresh destination and an existing installation. Mocked results do not establish
native compatibility or hosted asset availability.

## Unsupported platforms

- Given an unrecognized OS or architecture, what exit status and actionable
  diagnostic should the user receive, and should the diagnostic identify the
  detected values and a supported installation alternative?
- Should rejection happen before any network request or destination change?
  Can a simulated unsupported OS and architecture independently verify this?
- How should a recognized OS/architecture with an incompatible libc or missing
  release asset be distinguished from an unsupported platform? Which cases
  require tests on native hosts rather than mocked platform detection?

## Download failure

- For a failed latest-version lookup, missing archive, missing verification
  sidecar, timeout, or interrupted transfer, what exit status and diagnostic
  should distinguish the failing step and suggest a useful next action?
- When a download fails or returns partial data, should verification and
  extraction be prevented, and should existing binaries remain byte-for-byte
  unchanged? Can injected failures verify each boundary independently?
- What retry policy, if any, is intended for transient errors, and how can a
  test distinguish a bounded retry from an indefinite hang or a false success?
- If a downloaded archive or sidecar is malformed or their checksums disagree,
  what should be reported, and can a test show that unverified bytes never
  reach extraction or installation?

## Installation failure

- If archive inspection or extraction fails, or a required binary is missing,
  empty, or not a regular file, what diagnostic and exit status are expected?
  What should remain in a fresh destination and in an existing installation?
- If the destination cannot be created, written, staged, or made executable,
  what should the user be told, and which temporary files should be cleaned up?
  How can each failure be injected without relying on host permissions?
- If replacement of either binary fails, is a partial upgrade acceptable or
  must both previous binaries be restored? How should a user detect and recover
  from the resulting state? Test failures at each replacement separately.
- Across all failure cases, should success output be absent and the process
  exit nonzero? What information may diagnostics expose without leaking secrets?

Once decisions are made, capture expected observations as tests and record real
release and host results in the [acceptance checklist](installer-acceptance.md).
A matching checksum alone does not establish publisher authenticity if both the
archive and sidecar can be replaced.
