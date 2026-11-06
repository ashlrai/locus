# Shell installer user journey

[Issue #169](https://github.com/ashlrai/locus/issues/169) requests a
one-command, verified release-archive install for machines without npm or Rust.
This guide describes the repository's [installer](../scripts/install.sh) and
the decisions a user faces. It does not establish a published one-command URL
or that a given release has the required assets. For established alternatives,
see [installation options](../README.md#install-options).

## Before installing

Obtain and review the script from a trusted checkout. It needs Bash, HTTPS
access to GitHub releases, `curl`, `tar`, `uname`, `mktemp`, `mkdir`, `chmod`,
`mv`, `rm`, `tr`, and either `sha256sum` or `shasum`. It needs a writable
destination, and `HOME` unless `--bin-dir` is supplied. Neither npm nor Rust
is needed. It does not edit shell profiles or require sudo.

The script selects macOS Apple Silicon or Intel, or Linux x86-64 GNU archives;
Linux libc is not detected. A selected archive may still be unavailable or
incompatible with the host. Other OS/architecture pairs are rejected; consult
the [platform matrix](installer-platform-matrix.md) and consider the
[source install](../README.md#install-from-source) if appropriate.

## Run the installer

From the checkout root, inspect the options and run the local script:

```bash
bash scripts/install.sh --help
bash scripts/install.sh
```

The default resolves the latest release once and installs `locus` and
`locus-mcp` into `~/.local/bin`. To select a release and destination instead,
replace the example version and path with the intended values:

```bash
bash scripts/install.sh --version v0.6.2 --bin-dir "$HOME/.local/bin"
```

Versions with or without the leading `v` are accepted. The selected release
must contain both the target archive and its `.sha256` sidecar. The script
checks the archive against that sidecar before extracting the two binaries;
this checks consistency, not publisher authenticity if both files are replaced.
It stages both files before replacing them, but the final renames are separate,
so an interrupted or failed upgrade can leave mixed versions. See the
[failure matrix](installer-failure-matrix.md) for recovery and limitations.

## Verify and start

If prompted, add the chosen directory to your shell's `PATH` yourself. Check
which executables the shell finds, then run both; compare their versions with
the intended release:

```bash
command -v locus
command -v locus-mcp
locus --version
locus-mcp --version
```

If another installation appears first on `PATH`, invoke the chosen binaries by
their full paths or adjust `PATH`. A version check confirms the binaries start,
not that provider access is configured or that a release is authentic. For a
sample first run, see the [30-second start](../README.md#30-second-start) and
[onboarding guide](onboarding.md). Quickstart creates local state; use a
disposable Locus home if you are only evaluating it.

For missing assets, checksum errors, or unsupported hosts, consult the
[failure cases](shell-installer-failure-cases.md). The [acceptance checklist](installer-acceptance.md)
distinguishes offline fixture tests from checks on actual releases and hosts.

## Uninstall questions

The shell installer has no uninstall option or installation manifest. Before
removing anything, decide:

- Which installation owns each binary? Check the chosen destination and
  `command -v` results; another copy may be managed by npm, Cargo, or Homebrew.
  Use that manager's removal process for its own installation.
- Were either of the two files in the chosen directory replaced after the
  shell install, and do other users or integrations still need them? Only remove
  files you have identified as the intended installation.
- Should local Locus state, bindings, sessions, or MCP client configuration be
  retained? Removing binaries is separate from removing configuration or
  revoking provider credentials. Review those independently; do not assume
  uninstalling the binaries cleans them up.
- Did you add a PATH entry for this install? Remove it separately if no longer
  needed. What uninstall command, ownership record, state retention policy, and
  upgrade/rollback guarantees should a future published installer provide?

These questions remain open for the proposed published journey; the local
script does not promise automated cleanup or rollback.
