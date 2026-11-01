# Shell installer prerequisites

[Issue #169](https://github.com/ashlrai/locus/issues/169) requests a
one-command shell installer that downloads and verifies a release archive, in
the shape of the direnv installer. Its goal is to work on machines without npm
or Rust, without a local source build. The issue does not specify the remaining
prerequisites or establish that a hosted installer is available.

## Prerequisites of the repository implementation

These describe [`scripts/install.sh`](../scripts/install.sh), not requirements
settled by the issue or proof that release assets are published:

- Bash and the commands `curl`, `tar`, `uname`, `mktemp`, `mkdir`, `chmod`,
  `mv`, `rm`, and `tr`, plus either `sha256sum` or `shasum`. Curl must support
  the script's HTTPS and TLS options, and tar must support its gzip and
  extraction options.
- HTTPS access to GitHub releases, with the selected archive and its `.sha256`
  sidecar available under the same release tag. The default `latest` selection
  also requires resolving the latest release URL.
- A writable temporary directory and installation destination, with space for
  the archive, extracted binaries, and staged files. The default destination is
  `~/.local/bin`; `--bin-dir` selects another directory and avoids requiring
  `HOME`. The user must add the destination to `PATH` if needed. The script
  does not require sudo or edit shell profiles.
- A compatible host: the script selects macOS arm64 or x86-64, or Linux
  x86-64 GNU. Linux needs a compatible glibc, which the script does not detect.
  Target selection alone does not prove the downloaded binaries will run.

The script installs both `locus` and `locus-mcp`. It checks the archive against
the downloaded SHA-256 sidecar before extracting; a checksum from the same
source does not authenticate the publisher if both files are replaced. See the
[platform matrix](installer-platform-matrix.md) and [failure matrix](installer-failure-matrix.md)
for supported selections and failure behavior. Offline fixture tests do not
prove hosted availability or native compatibility; use the
[acceptance checklist](installer-acceptance.md) for release and host evidence.

## Open questions from the issue

- Which operating systems, architectures, libc versions, and shell utilities
  must the published command support? Are WSL, Linux ARM, musl, or Windows in
  scope, and which native hosts must be tested?
- Where will the command, archives, and verification material be hosted, and
  which versions and targets must have assets? What network or proxy access is
  acceptable?
- What verification and publisher authentication are required? Is a checksum
  delivered alongside the archive sufficient for the intended trust model?
- What destination, permissions, PATH setup, version selection, and upgrade or
  rollback behavior should be promised? Must both binaries be installed?
- What constitutes a successful first run and release acceptance, including
  behavior when a prerequisite or release asset is missing?

The [requirements record](installer-requirements.md) separates the issue's
request from implementation choices. These questions remain open until a
published contract and supporting evidence settle them.
