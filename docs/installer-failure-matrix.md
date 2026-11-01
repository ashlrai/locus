# Shell installer failure matrix

[Issue #169](https://github.com/ashlrai/locus/issues/169) requests a shell
installer for verified release archives on machines without npm or Rust. This
matrix describes the repository's [`scripts/install.sh`](../scripts/install.sh),
not the npm wrappers or a published installer URL. It is a troubleshooting and
acceptance aid for the [installer checklist](installer-acceptance.md), not proof
that any particular release asset or hosted entry point exists.

Failures return a nonzero status and do not print the final `Installed locus and
locus-mcp` success message. Explicit installer errors appear on stderr with the
prefix `locus installer:`; underlying tools can also print errors. “Preserved”
means existing destination binaries are unchanged in an otherwise stable
filesystem, because the failure occurs before their final replacement.

## Download and verification

| Condition | User-visible outcome and destination state | Evidence / follow-up |
| --- | --- | --- |
| Missing curl, tar, required utility, or both SHA-256 tools | Names the missing command or requires `sha256sum` or `shasum`; no download or replacement. | Offline missing curl, tar and checksum-tool tests. |
| Unsupported OS/architecture | Reports `unsupported platform` and suggests a source install; no download or replacement. | Offline platform tests; native compatibility still needs the checklist's real-machine checks. |
| Latest redirect fails, points outside the expected release tag URL, or contains an invalid version | Reports inability to resolve, unexpected URL, or invalid latest version; existing binaries preserved. | Offline redirect and download tests. |
| Archive or `.sha256` download fails, including a missing release asset | Reports `archive download failed; nothing installed` or `checksum download failed; nothing installed`; existing binaries preserved. curl may also report the underlying error. | Offline simulated failures; actual release asset availability is unverified. |
| Sidecar empty, malformed, has the wrong filename, or has multiple records | Reports an invalid sidecar or that it must contain exactly one record; tar is not invoked and existing binaries are preserved. | Offline sidecar tests. |
| SHA-256 mismatch | Reports `SHA-256 checksum mismatch; nothing installed`; tar is not invoked and existing binaries are preserved. | Offline mismatch test. A matching sidecar verifies consistency, not publisher authenticity if both files are replaced. |
| Invalid option/version or missing `HOME` with no `--bin-dir` | Reports the invalid input or asks for `--bin-dir`; no download or replacement. | Offline argument and HOME tests. |

## Archive inspection and extraction

These cases assume the archive and sidecar were downloaded and the checksum
matched. An archive that fails checksum verification never reaches tar.

| Condition | User-visible outcome and destination state | Evidence / follow-up |
| --- | --- | --- |
| Corrupt archive or missing required member | Reports `archive must contain locus and locus-mcp`; tar may print details. Existing binaries preserved. | Offline corrupt and missing-member tests. |
| Binary member is a link or directory, or appears more than once | Reports that binaries must be regular files or occur exactly once; existing binaries preserved. | Offline link, directory and duplicate tests. |
| Required member is empty or cannot be streamed | Reports `archive contains an empty` binary or `could not unpack` it; existing binaries preserved. | Offline empty-member and simulated streaming-failure tests. |
| Unrelated absolute or traversal archive entries | Only the two named binaries are streamed; unrelated entries are not extracted. This alone does not cause an error. | Absolute-path fixture is tested; traversal fixture and real release archives remain **unverified**. |

## Destination and replacement

| Condition | User-visible outcome and destination state | Evidence / follow-up |
| --- | --- | --- |
| Either binary destination is an existing symlink or directory | Reports `refusing symlink or directory destination`; neither binary is replaced. | Offline tests cover the second binary and check the first remains unchanged. |
| Install directory is a file or cannot be created | Reports `cannot create install directory; choose a writable --bin-dir`, potentially with a `mkdir` error; no binary replacement. | Offline existing-file test; actual permission failures remain **unverified**. |
| Destination staging directory cannot be created, or staging move/chmod fails | Underlying utility reports an error; no final replacement has started. Temporary cleanup is attempted. | Exact diagnostics and permission-dependent behavior are **unverified**. |
| Final rename fails | Underlying `mv` reports an error; there is no rollback. The first binary may already have been replaced when the second fails, leaving mixed versions. Temporary cleanup is attempted. | Offline injected second-rename failure checks the partial upgrade, failure status and cleanup; other filesystem failures are **unverified**. |

After a partial upgrade, fix the destination problem and rerun the installer,
then check both binary versions. The script stages files before replacing them,
but its two final renames are separate. Cleanup can itself fail if filesystem
permissions or availability change; it is not a transaction guarantee.

Run `bash scripts/test-install.sh` for disposable offline fixtures. Those tests
mock downloads and platform detection and do not establish network availability,
release authenticity, published assets, or native execution. Record those
separately using the [acceptance checklist](installer-acceptance.md).
