# Archive installer acceptance cases

[Issue #169](https://github.com/ashlrai/locus/issues/169) requests a one-command
shell path that downloads and verifies a release archive on machines without npm
or Rust. These cases evaluate the current [repository installer](../scripts/install.sh);
they do not assert that a hosted command or release assets exist. The issue does
not specify a platform matrix or checksum trust model. The cases below describe
current implementation choices, not additional promises from the issue.

## Setup and evidence

Use a disposable account or VM and destination. For native acceptance, remove
npm, Node.js, Cargo and Rust from the available tools; Bash, curl, tar, standard
utilities, and `sha256sum` or `shasum` are still needed. Use an actual candidate
release and its matching archive and sidecar. Record the installer revision,
release tag, OS, architecture, libc where applicable, command, exit status and
result for each case. Do not record credentials. If a hosted one-command entry
point is proposed, run that exact command as a separate check; running the local
script does not prove that entry point exists.

For reproducible offline evidence, run `bash scripts/test-install.sh` from the
repository. It restricts PATH, mocks platform detection and downloads, and uses
synthetic archives. It needs no npm or Rust, but cannot establish published asset
availability or native binary compatibility. Run native checks for macOS arm64,
macOS x86-64, and Linux x86-64 glibc separately; simulated `uname` values do not
substitute for them. Linux musl compatibility is not established.

In failure cases below, start with two distinctive existing destination files
and compare their bytes afterward. Unless stated otherwise, failures occur before
replacement: expect a nonzero status, no final success message, unchanged files,
and cleanup of temporary files. Use controlled fixtures, never real releases or
installations, to inject faults.

## Success cases

| Setup and action | Observable acceptance result | Evidence |
| --- | --- | --- |
| Install a valid matching archive and sidecar on each supported native host without npm or Rust. | Exit zero; executable `locus` and `locus-mcp` appear in `~/.local/bin`; both run and identify the selected release. No sudo, prompt, source build, or shell profile change. | Offline fixture covers installation; native execution and assets require real-host checks. |
| Use latest, then a specific version with and without `v`. | Latest resolves once; archive and sidecar use the same tag. Explicit versions select that tag. | Offline fixture. |
| Set `--bin-dir` to a writable path containing spaces; repeat over existing files. | Both binaries match the new archive and are executable; repeat succeeds; staging files are removed. | Offline fixture. |
| Install with destination absent from, then present in PATH. | Guidance to add it appears only when absent; profiles are unchanged. | Offline fixture. |
| Remove `sha256sum` but provide `shasum`; use a valid sidecar. | Verification uses `shasum -a 256` and installation succeeds. | Offline fixture. |
| Use an archive with unrelated absolute or `../` entries. | Both expected binaries install; unrelated entries are not extracted. | Offline fixtures. |

## Failure cases

| Setup and action | Observable acceptance result | Evidence |
| --- | --- | --- |
| Supply an unknown option, missing option value or invalid version; request `--help`. | Invalid input fails before download; help succeeds without download. | Offline fixture. |
| Omit HOME with no `--bin-dir`; then specify a destination. | Default fails before download with guidance; explicit destination succeeds. | Offline fixture. |
| Remove curl, tar, or both checksum tools; select an unsupported OS/architecture. | Error identifies the missing prerequisite or unsupported platform; no download or replacement. | Offline fixture. |
| Fail latest resolution, use an invalid redirect/tag, or fail either asset download. | Error identifies resolution or download failure; no archive extraction or replacement. | Offline fixture. |
| Supply an empty, malformed, wrong-name, duplicate or mismatched sidecar. | Verification fails before tar reads the archive; no replacement. | Offline fixture. |
| Supply a corrupt archive or omit, empty, duplicate, link, or replace a required binary member with a directory; simulate extraction failure. | Installer rejects the archive or extraction; neither destination is replaced. | Offline fixture. |
| Make either destination a symlink or directory, or make the install directory a file. | Installer rejects it without replacing the other binary or modifying the symlink target. | Offline fixture checks the second destination; verify both names on a native host. |
| Cause an unwritable destination or a staging failure. | Nonzero status, no success message, and no final replacement; cleanup is attempted. | Permission-dependent behavior requires a controlled native check. |
| Inject failure on the second final rename. | Nonzero status and cleanup; the first binary may already be replaced and the second remains old. Fix the cause and rerun, then check both versions. | Offline fixture. The two renames are not an atomic pair and rollback is not guaranteed. |

A matching SHA-256 sidecar checks consistency, not publisher authenticity if
both archive and sidecar are replaced. See the [failure matrix](installer-failure-matrix.md)
for diagnostics and the [acceptance checklist](installer-acceptance.md) for
release and real-machine evidence. Mark any unrun native case as unverified.
