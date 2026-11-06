# Shell installer platform matrix

[Issue #169](https://github.com/ashlrai/locus/issues/169) requests a verified
release-archive installer for machines without npm or Rust. This matrix describes
the repository's [shell installer](../scripts/install.sh), not a guarantee that
a hosted entry point or assets exist for a particular release. Its platform
selection is implemented; native compatibility and release availability must be
checked separately using the [acceptance checklist](./installer-acceptance.md).

## Selected release targets

The installer maps `uname -s` and `uname -m` to these archives. Each archive
is named `locus-<target>.tar.gz` and needs a matching `.sha256` sidecar from
the same release tag. Both `locus` and `locus-mcp` are installed.

| Host OS / architecture reported by `uname` | Release target | Qualification |
| --- | --- | --- |
| macOS `arm64` / `aarch64` | `aarch64-apple-darwin` | Apple Silicon |
| macOS `x86_64` / `amd64` | `x86_64-apple-darwin` | Intel |
| Linux `x86_64` / `amd64` | `x86_64-unknown-linux-gnu` | Compatible glibc required; libc is not detected |

These mappings are specified in the [installer](../scripts/install.sh) and
the [npm installer documentation](../npm/README.md#shell-installer-requirements-issue-169).
The [offline installer tests](../scripts/test-install.sh) exercise the aliases
with mocked platform detection and downloads, not native release binaries.
The [benchmark record](./BENCHMARKS.md) lists these three assets for v0.5.1;
it does not establish asset availability for later releases.

## Prerequisites and alternatives

- Bash, `curl`, `tar`, `uname`, `mktemp`, `mkdir`, `chmod`, `mv`, `rm`, `tr`,
  and either `sha256sum` or `shasum` are needed, plus HTTPS access to GitHub
  releases and a writable destination. The default is `~/.local/bin`; an
  explicit `--bin-dir` avoids requiring `HOME`. Add the destination to `PATH`
  yourself if needed. No npm, Rust, sudo, or shell profile edits are required.
- The checksum verifies archive bytes against the downloaded sidecar; it is not
  a publisher signature. Missing or invalid artifacts fail before installation.
  See the [acceptance checklist](./installer-acceptance.md) for failure and
  upgrade limits, including the separate final renames of the two binaries.
- The shell installer does not fall back to Cargo. For other installation
  paths, see [Install options](../README.md#install-options). A source build
  needs Rust and Cargo 1.89 or newer, as documented in the [MSRV guide](./MSRV.md).
  The npm wrappers have their own prebuilt selection and Cargo fallback; their
  behavior is described in the [CLI](../npm/README.md) and
  [MCP](../npm-mcp/README.md) package documentation.

## Unresolved decisions and evidence gaps

These are open questions or unverified outcomes, not promises of support:

| Area | Current behavior and decision still needed | Source |
| --- | --- | --- |
| Linux ARM, Windows, other OS/architectures | The shell installer rejects them before download. Decide whether to produce and support additional archives and installer mappings; a source build is an alternative, not a guarantee on every host. | [installer](../scripts/install.sh), [acceptance checklist](./installer-acceptance.md) |
| Linux musl and glibc baseline | Linux x86-64 selects the GNU archive without checking libc. Decide whether to support musl, and establish and test a minimum compatible glibc version. Selection alone does not prove execution. | [npm requirements](../npm/README.md#shell-installer-requirements-issue-169), [acceptance checklist](./installer-acceptance.md) |
| WSL | No separate WSL target or native test is specified. Decide what WSL distributions and libc combinations to support and test; a Linux `uname` may select the GNU archive. | [installer](../scripts/install.sh), [acceptance checklist](./installer-acceptance.md) |
| Hosted entry point and release coverage | The repository script does not establish a published one-command URL or matching assets for every release. Decide and validate the delivery path, then record actual asset and native execution results per release and host. | [issue #169](https://github.com/ashlrai/locus/issues/169), [acceptance checklist](./installer-acceptance.md) |

Do not mark a platform accepted solely from the offline tests. Record the tag,
installer revision, host, commands and native results as the checklist directs.
