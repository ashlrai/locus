# locus-cli

<!-- fresh:begin install -->
```bash
npm install -g locus-cli@0.5.2 @ashlrai/locus-mcp@0.5.2
```
<!-- fresh:end -->

npm wrapper for the [Locus](https://github.com/ashlrai/locus) CLI — identity plane for coding agents.

**Wrong account, impossible.** Pin a binding; every command is hard-scoped to that tenant until you re-pin.

## Install

```bash
npm install -g locus-cli
# or
npx locus-cli --help
```

Requires Node ≥ 16. Without an override, the wrapper selects a binary as follows:

1. It checks `~/.locus/bin/locus` first. A binary matching the wrapper's packaged
   version is reused. A newer binary, or one whose version cannot be determined,
   is preserved and the wrapper refuses to run; it does not fall through to
   another installation. An older cached binary can be upgraded.
2. If the cache is absent or older, it checks for an existing native binary on
   `PATH` (or in `~/.cargo/bin`). It reuses one matching or newer than the
   packaged version; an older or unversioned candidate is not selected.
3. Otherwise it downloads the GitHub release matching the wrapper's packaged
   version into `~/.locus/bin` after verifying its SHA-256 sidecar. If the
   checksum cannot be retrieved or parsed, or does not match, installation stops
   with an error and does not fall back to Cargo; retry after checking the
   release sidecar. The CLI's eager postinstall also fails on checksum errors;
   when eager installation is deferred, the error appears on first use. The
   sidecar detects corruption, but is not a publisher signature. If another
   download or extraction error occurs, or there is no prebuilt release for the
   platform (currently Linux arm64 and Windows), it builds
   with `cargo install --git https://github.com/ashlrai/locus locus-cli --locked`.
   This fallback requires Rust 1.89 or newer ([rustup](https://rustup.rs)) — the
   workspace's `rust-version` — and can take several minutes.

Versions are compared using SemVer precedence, so prereleases precede their
stable release and build metadata does not affect ordering. The wrapper probes
the binary with `--version` first and uses its adjacent `.version` file only if
the probe fails or is unrecognized. It checks again before replacing a cache
entry. The Cargo fallback builds from the Git repository's default branch, not
the npm package's release tag, and compares the built binary's actual version
with the cache. It uses a temporary private install root; it does not replace
binaries in Cargo's install directory. If the built version is unknown, it will
not replace an existing cache entry without an override.

On supported POSIX platforms npm postinstall may download and link the matching
release immediately. It also preserves a newer or unversioned cached binary and
finishes npm installation successfully, reporting the refusal. The CLI release
archive can include `locus-mcp`; that companion is installed best-effort and a
newer or unversioned cached companion is preserved independently.

To intentionally replace a protected cache entry, run the JavaScript wrapper
with a leading `--force`, for example `locus --force --version`, or run
`node install.js --force` from the installed `locus-cli` package directory. The
wrapper consumes this leading flag; flags after a command, such as `locus leave
--force`, belong to the native CLI and never authorize a downgrade. npm's own
`--force` is not an override. If the native binary has already been linked in
place of the wrapper, reinstall the npm package to restore the JavaScript wrapper
before using either override.

## Shell installer requirements (issue #169)

The proposed shell installation path is separate from the npm wrapper above.
This checkout includes `scripts/install.sh`; see the [source install options](https://github.com/ashlrai/locus#install-options)
for how to inspect and run it. The requirements below describe that path, not
a change to `npm install`, its cache selection, or its Cargo fallback.

The installer should select the release archive from the host OS and machine
architecture (`uname -s` and `uname -m`):

| Host | Release archive |
| --- | --- |
| macOS arm64 / aarch64 | `locus-aarch64-apple-darwin.tar.gz` |
| macOS x86_64 / amd64 | `locus-x86_64-apple-darwin.tar.gz` |
| Linux x86_64 / amd64, glibc | `locus-x86_64-unknown-linux-gnu.tar.gz` |

These are the release assets for the supported platforms; Linux arm64,
Windows, other architectures, and Linux systems without a compatible glibc
are not supported by this prebuilt path. `uname` does not establish libc
compatibility, so selecting the Linux archive does not guarantee it will run
on a musl-based system. Use a source installation on unsupported platforms.

For a requested version (or a latest release resolved once to a version), the
installer should download that version's matching archive and `.sha256`
sidecar, verify the checksum before unpacking, and install both `locus` and
`locus-mcp`. It should fail with a useful error for an unsupported OS or
architecture, missing tools or assets, failed downloads, an invalid or
mismatched checksum, or an invalid archive; it should not silently select a
different platform or fall back to Cargo. Validation failures must leave existing
binaries intact. A checksum detects corruption; it is not a publisher signature.

## Quick start

```bash
locus quickstart                 # sample bindings, pin, whoami, doctor
eval "$(locus hook bash)"        # or zsh; exports the control capability
locus whoami
locus exec -- env | grep LOCUS_
```

Step by step instead of `quickstart`: `locus init --with-samples`, then
`eval "$(locus hook bash)"`, then `locus pin personal`. Without the hook line,
`pin` fails with `LOCUS_CONTROL_CAPABILITY is required`.

## Related

- MCP server: [`@ashlrai/locus-mcp`](https://www.npmjs.com/package/@ashlrai/locus-mcp)
- Pairs with Phantom Secrets — the secret plane to Locus's identity plane: https://github.com/ashlrai/locus#how-locus-plugs-into-phantom
- Source / Homebrew: https://github.com/ashlrai/locus
