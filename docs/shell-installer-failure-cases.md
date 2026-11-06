# Shell installer failure cases

[Issue #169](https://github.com/ashlrai/locus/issues/169) proposes a
one-command installer. This guide describes the repository's
[`scripts/install.sh`](../scripts/install.sh); it does not establish a hosted
installer URL or that any particular release assets exist. Explicit installer
errors go to stderr with a `locus installer:` prefix; curl and tar may also
report details. Failures below occur before destination binaries are replaced,
assuming the filesystem does not change concurrently.

## Unsupported platforms

The script checks `uname -s` and `uname -m` before downloading. It accepts
macOS arm64/aarch64 and x86_64/amd64, and Linux x86_64/amd64. Other
combinations produce `unsupported platform: OS/ARCH; use a source install`.
Check the host architecture and consider a source install using the
[installation instructions](../README.md#install-from-source). Linux x86-64 selects
the GNU archive without checking libc; selection does not prove that a binary
will run on the host. See the [platform matrix](installer-platform-matrix.md)
for compatibility limits.

## Download and verification failures

- With the default `latest`, a failed redirect produces `could not resolve the
  latest release`; an unexpected redirect or invalid tag also stops the script.
- A failed archive or checksum download reports `archive download failed;
  nothing installed` or `checksum download failed; nothing installed`. Check
  connectivity and whether the requested release has both target assets.
- An invalid or mismatched checksum sidecar stops the script before tar reads
  the archive. Check the release assets; a matching SHA-256 sidecar establishes
  consistency, not publisher authenticity if both files were replaced.

## Archive inspection and extraction failures

After checksum verification, tar must find both required binary members. A
corrupt archive or missing member reports `archive must contain locus and
locus-mcp`. Links, directories, duplicate members, and empty binaries are also
rejected. If streaming a member fails after inspection, the script reports
`could not unpack locus` or `could not unpack locus-mcp`. Check tar's error and
the candidate archive; do not bypass checksum or member validation.

The [failure matrix](installer-failure-matrix.md) covers other errors, including
destination failures and the possibility of a partial upgrade during the two
final renames. The [acceptance checklist](installer-acceptance.md) describes
real-machine checks. The offline `bash scripts/test-install.sh` suite uses
synthetic archives and mocked downloads and platforms; it does not prove that
release assets, a hosted entry point, or native compatibility exist.
