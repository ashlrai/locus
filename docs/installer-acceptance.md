# Shell installer acceptance checklist

[Issue #169](https://github.com/ashlrai/locus/issues/169) calls for a
one-command installation from a verified release archive on machines without
npm or Rust. This checklist is for evaluating the repository's
[`scripts/install.sh`](../scripts/install.sh) and a candidate release. It is
not evidence that a hosted one-command entry point or assets exist for every
release. Record the release tag, installer revision, platform, command, and
result for each real-machine check; leave unverified items unchecked.

## Scope and prerequisites

- [ ] Confirm the candidate release provides the matching `locus-<target>.tar.gz`
  and `.sha256` sidecar for each target being accepted. A missing asset is a
  failure for that candidate, not proof that all releases have assets.
- [ ] On a clean machine without npm, Node.js, Cargo, or Rust, install both
  `locus` and `locus-mcp` from the archive using Bash. Confirm no source build,
  npm, sudo, credentials, or interactive prompt is needed. Bash, curl, tar,
  common shell utilities, and `sha256sum` or `shasum` are still required.
- [ ] If a hosted one-command entry point is proposed, test that exact entry
  point separately. The repository script alone does not establish its URL or
  availability.

## Platform matrix

Test with the native tools and actual release binaries on each platform; mocked
`uname` checks do not establish binary compatibility. The installer selects:

| Host | Archive target | Real-machine result |
| --- | --- | --- |
| macOS Apple Silicon (`arm64` / `aarch64`) | `aarch64-apple-darwin` | [ ] |
| macOS Intel (`x86_64` / `amd64`) | `x86_64-apple-darwin` | [ ] |
| Linux x86-64 with glibc (`x86_64` / `amd64`) | `x86_64-unknown-linux-gnu` | [ ] |

- [ ] Verify unsupported OS/architecture combinations, including Linux ARM and
  Windows, fail clearly before downloading. Linux musl compatibility is not
  established by the Linux x86-64 selection; do not count it as supported.
- [ ] Run both installed binaries on each supported host (for example,
  `locus --version` and `locus-mcp --version`) and confirm the selected release
  version. An offline fixture is not a substitute for this smoke test.

## Installation and upgrade

- [ ] Check the default latest-release resolution and an explicit version with
  and without `v`; the archive and sidecar must come from the same resolved tag.
- [ ] Confirm the default destination is `~/.local/bin`, both files are
  executable, and a custom `--bin-dir` works, including a path with spaces.
  An explicit destination works without `HOME`; the default fails clearly
  when `HOME` is unset.
- [ ] With the destination absent from `PATH`, check that the installer gives
  guidance; with it present, check that it does not give unnecessary guidance.
  Confirm shell profiles and existing credentials are untouched.
- [ ] Repeat an installation and upgrade over existing binaries. Confirm both
  are replaced with the intended release and temporary files are cleaned up.
  The two final renames are separate: do not claim an atomic two-binary upgrade
  or preservation after every possible destination write/rename failure.

## Verification and failure cases

Use disposable directories and controlled fixtures; never corrupt real release
assets or an existing installation. For failures before replacement, seed both
destination binaries and verify their bytes remain unchanged.

- [ ] Missing curl, tar, or both checksum tools fails before archive extraction;
  exercise the `sha256sum` and `shasum -a 256` paths separately.
- [ ] A failed latest redirect, invalid redirect/tag, failed archive download,
  or missing sidecar fails without replacing the seeded binaries.
- [ ] Empty, malformed, wrong-filename, duplicate, or mismatched checksum
  sidecars fail before tar reads the archive. The SHA-256 sidecar checks
  consistency of bytes with the sidecar; it is not a publisher signature and
  cannot establish authenticity if both files are replaced.
- [ ] Reject corrupt archives and missing, empty, linked, directory, or
  duplicate binary members before replacing the seeded binaries. Verify that
  unrelated archive entries, including absolute or traversal paths, are not
  extracted to the filesystem.
- [ ] Reject existing symlink or directory destinations for either binary and
  a non-directory install path. Check errors for unwritable destinations and
  cleanup after failures; do not assume rollback if a final rename fails.
- [ ] Reject invalid options and versions without downloading; check `--help`.

## Evidence

Run `bash scripts/test-install.sh` from the repository for offline regression
coverage. It uses mocked downloads and platform detection and synthetic archive
members, so it does not prove network availability, published assets, or native
execution. Record its result alongside the real-machine matrix and candidate
release checks above; neither kind of evidence substitutes for the other.
