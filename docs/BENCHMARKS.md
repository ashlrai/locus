# Benchmarks

Measured on 2026-10-26 UTC. One machine, one trial for each install, and 21 runs with the first dropped for each latency series. Every number below comes from those runs or from the public GitHub API on the same date. Method, peers, and the feature notes are in this file. The first-run log and the follow-up issues are in [FIRST-RUN.md](./FIRST-RUN.md).

## What was compared

Locus pins one binding and keeps later CLI commands and MCP tools on that binding. The README comparison table names [direnv](https://github.com/direnv/direnv) for directory environment variables and 1Password `op run` for one command with resolved secrets. These runs use direnv for the first, and [sops](https://github.com/getsops/sops) for the second, because `sops exec-env` decrypts a file into one child process. 1Password CLI was not installed.

| Repo | License | Release used |
|---|---|---|
| [ashlrai/locus](https://github.com/ashlrai/locus) | MIT | npm `0.5.1` and git `e969d9c4` |
| [direnv/direnv](https://github.com/direnv/direnv) | MIT | v2.38.2 |
| [getsops/sops](https://github.com/getsops/sops) | MPL-2.0 | v3.13.3 |

The sops age path also installed [FiloSottile/age](https://github.com/FiloSottile/age) v1.3.2 (BSD-3-Clause), the key tool for that flow, not a third peer.

## Machine

- Linux 6.12 x86_64, 4 vCPU, 15.6 GiB RAM.
- Node v22.23.4, npm 10.9.9.
- rustc 1.99.1 (stable) and 1.83.0 (the machine's default toolchain).
- No Homebrew, no pre-filled cargo registry, and no `locus` on `PATH` before each install. Each install used a fresh `HOME`.

## Method

**First useful result.** `locus quickstart` or `locus whoami` prints the sample binding `personal`. For direnv, `direnv exec` prints the value exported by an allowed `.envrc`. For sops, `sops exec-env` prints the value decrypted from an age file.

**Install memory.** A sampler summed resident pages (`/proc/<pid>/statm`) for the process and its children. The npm install and the failed cargo install sampled every 0.01 s. The successful cargo install sampled every 0.25 s, so a spike between samples is missing from that peak. The sampler does not report `wait4` `ru_maxrss`, because `poll` reaps the child.

**Latency and RSS.** 21 runs, first dropped. `os.wait4` supplies wall time and `ru_maxrss`. On this Linux, `ru_maxrss` is kilobytes. A check that allocated a 40 MiB string (`'x'*(40*1024*1024)`) reported `rss_kb` 50480. Short commands were started with stdin closed or from a script whose stdin was not a terminal.

**Core operation.** After a live pin, `locus whoami`, `locus exec -- true`, and `locus status --oneline`. Peers: `direnv exec <dir> true` after `direnv allow`, and `sops exec-env <file> true` with `SOPS_AGE_KEY_FILE` set. `sops --version` was not the latency command: it printed a warning that the command checks the network for a newer release.

**Scrub check.** The parent set `GH_TOKEN`, `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, and `DIRENV_BENCH` to synthetic values. A child script printed `set` or `unset` for each name and whether `HOME` matched the parent. Values are not recorded here.

**Which Locus binary.** npm `0.5.1` downloads the GitHub release `v0.5.1` (commit `89f12eb`). The cargo success run built `e969d9c4` (main after the tag).

## Install to first useful result

| Path | Command | Wall | Exit | Tree peak RSS | First useful result |
|---|---|---|---|---|---|
| Locus npm | `npm install -g locus-cli @ashlrai/locus-mcp` (resolved 0.5.1) | 0.775 s | 0 | 182415360 bytes | quickstart in that shell, 0.123 s, stdout contained `whoami personal` |
| Locus cargo, default toolchain | `cargo install --git ... locus-cli locus-mcp --locked` with rustc 1.83.0 | 1.924 s | 101 | 160849920 bytes | no binary |
| Locus cargo, stable | same command, `RUSTUP_TOOLCHAIN=stable`, rustc 1.99.1, fresh cargo home | 86.488 s | 0 | 1485778944 bytes | `locus quickstart` on that binary, 0.138 s, exit 0, stdout contained `whoami personal` |
| direnv | [install.sh](https://direnv.net/install.sh) with `PATH` starting at a writable directory | 0.530 s | 0 | not sampled | `direnv allow` 3 ms, then `direnv exec` 19 ms, exit 0 |
| sops | `curl` of `sops-v3.13.3.linux.amd64` | 0.534 s | 0 | not sampled | see the age steps below |
| age | download and unpack v1.3.2 | 0.436 s | 0 | not sampled | `age-keygen`, `sops encrypt`, and `sops exec-env` together 0.040 s, exit 0 |

The 60 second target is met by the npm install plus quickstart. It is two commands. The cargo install on stable is one command and took 86.488 s before quickstart. The default-toolchain cargo command produced no binary. Homebrew was not run.

Cargo detail for the 86.488 s run: `locus-cli` printed `Finished release` in 50.96 s, then `locus-mcp` compiled `locus-core` again and printed `Finished` in 1 m 26 s. Binaries: `locus` 12825264 bytes, `locus-mcp` 5493408 bytes.

The 1.924 s failure stopped while parsing `zeroize_derive` 1.5.0. Cargo's text: feature `edition2024` is required, and that feature is not stabilized in Cargo 1.83.0. `cargo install --git` used the default toolchain. The repo file `rust-toolchain.toml` sets `channel = "stable"` and did not change that default, because the command's working directory was outside the repo.

npm release binary sizes: `locus` 10011528 bytes, `locus-mcp` 4321256 bytes, both stripped ELF files.

## Core-operation latency

HEAD binary `e969d9c4`, pin live, 20 runs after one warmup, every exit 0. direnv used `HOME` that already held the allow file. An earlier direnv series exited 1 on every run with `.envrc is blocked` because `HOME` did not match that allow file. Those exit 1 times are not in the table.

| Command | Min | Median | Max | `ru_maxrss` min / median / max (KiB) |
|---|---|---|---|---|
| `locus whoami` | 0.020 s | 0.020 s | 0.021 s | 13400 / 13528 / 13528 |
| `locus exec -- true` | 0.050 s | 0.051 s | 0.051 s | 13528 / 13528 / 13528 |
| `locus status --oneline` | 0.020 s | 0.020 s | 0.021 s | 13528 / 13528 / 13528 |
| `direnv exec <dir> true` | 0.015 s | 0.016 s | 0.016 s | 13648 / 13648 / 13648 |
| `sops exec-env <file> true` | 0.010 s | 0.011 s | 0.014 s | 34064 / 34448 / 35472 |

## Ambient environment

| Launcher | Four parent variables | `HOME` | Exit |
|---|---|---|---|
| plain Python | set | same | 0 |
| `locus exec` (HEAD, live pin) | unset | rewritten | 0 |
| `direnv exec` after allow | set | same | 0 |
| `sops exec-env` | set | same | 0 |
| `sops exec-env --pristine` | unset | same | 0 |

The locus line's stderr included `scrubbed 41 ambient` and `resolved 0 secret(s)`, then three warnings that the sample providers' injection variables were unset (`SUPABASE_ACCESS_TOKEN`, `GH_TOKEN,GITHUB_TOKEN,GITHUB_PERSONAL_ACCESS_TOKEN`, `VERCEL_TOKEN`). Exit stayed 0.

`locus exec --expect acme` while `personal` was pinned exited 1. stderr: `pin_mismatch: expected binding 'acme' but 'personal' is pinned; nothing was run. Run: locus enter acme`.

## Release assets

Locus prebuilt assets on the v0.5.1 release are three platform archives: `locus-aarch64-apple-darwin.tar.gz`, `locus-x86_64-apple-darwin.tar.gz` and `locus-x86_64-unknown-linux-gnu.tar.gz` (each with a `.sha256`). The measurement machine is `x86_64-unknown-linux-gnu`, which has an asset. Linux arm64 and Windows are not in that asset list.
