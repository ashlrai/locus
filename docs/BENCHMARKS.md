# Benchmarks

Measured on 2026-10-26 UTC. One machine, one trial for each install, and 21 runs with the first dropped for each latency series. Every number below comes from those runs or from the public GitHub API on the same date. Method, peers, and the feature notes are in this file. The first-run log and the follow-up list are in [FIRST-RUN.md](./FIRST-RUN.md).

## What was compared

Locus pins one binding and keeps later CLI commands and MCP tools on that binding. The README comparison table names [direnv](https://github.com/direnv/direnv) for directory environment variables and 1Password `op run` for one command with resolved secrets. These runs use direnv for the first, and [sops](https://github.com/getsops/sops) for the second, because `sops exec-env` decrypts a file into one child process. 1Password CLI was not installed.

| Repo | License | Stars | Last push | Release used |
|---|---|---|---|---|
| [ashlrai/locus](https://github.com/ashlrai/locus) | MIT | 2 | 2026-10-26T02:22:18Z | npm `0.5.1` and git `e969d9c4` |
| [direnv/direnv](https://github.com/direnv/direnv) | MIT | 15521 | 2026-10-12T20:56:43Z | v2.38.2, published 2026-10-10T08:41:29Z |
| [getsops/sops](https://github.com/getsops/sops) | MPL-2.0 | 23505 | 2026-10-20T04:34:01Z | v3.13.3, published 2026-07-23T05:27:57Z |

Stars, licenses, and push times are from `https://api.github.com/repos/<owner>/<repo>` on 2026-10-26. Release times are from `https://api.github.com/repos/<owner>/<repo>/releases/tags/<tag>`. The sops age path also installed [FiloSottile/age](https://github.com/FiloSottile/age) v1.3.2 (published 2026-08-29T17:55:36Z, BSD-3-Clause, 24060 stars, last push 2026-08-29T17:55:36Z). Age is the key tool for that sops flow, not a third peer.

## Machine

- Linux 6.12.94+ x86_64, 4 vCPU, `/proc/cpuinfo` model `Intel(R) Xeon(R) Processor`.
- `MemTotal` 16387280 kB. At the start of the successful cargo install, `MemAvailable` was 5061996 kB. `/proc/vmstat` had `balloon_inflate` 2621440 and `balloon_deflate` 0. Those counters are 4 KiB pages, so 10737418240 bytes were still inflated.
- Node v22.23.4 and npm 10.9.9 from nvm (`/home/ubuntu/.nvm/versions/node/v22.23.4`). That directory was first on `PATH` for the npm install, so `npm config get prefix` was that directory.
- Rustup already had `stable` (rustc 1.99.1, 58cf8f9c3, 2026-10-13; cargo 1.99.1, 5f94df478, 2026-08-27) and the default toolchain 1.83.0 (rustc 90b35a623, 2024-11-26; cargo 5ffbef321, 2024-10-29). The rustup download is not in the times.
- `command -v brew` was empty. `/usr/bin/time` was absent. Wall clock is `time.perf_counter` around the process, or `date +%s%N` where noted.
- No Homebrew, no pre-filled cargo registry, and no existing `locus` on `PATH` before each install. Each install used a fresh `HOME`.

## Method

**First useful result.** `locus quickstart` or `locus whoami` prints the sample binding `personal`. For direnv, `direnv exec` prints the value exported by an allowed `.envrc`. For sops, `sops exec-env` prints the value decrypted from an age file.

**Install memory.** A sampler summed resident pages (`/proc/<pid>/statm`) for the process and its children. The npm install and the failed cargo install sampled every 0.01 s. The successful cargo install sampled every 0.25 s, so a spike between samples is missing from that peak. The sampler does not report `wait4` `ru_maxrss`, because `poll` reaps the child.

**Latency and RSS.** 21 runs, first dropped. `os.wait4` supplies wall time and `ru_maxrss`. On this Linux, `ru_maxrss` is kilobytes. A check that allocated a 40 MiB string (`'x'*(40*1024*1024)`) reported `rss_kb` 50480. Short commands were started with stdin closed or from a script whose stdin was not a terminal.

**Core operation.** After a live pin, `locus whoami`, `locus exec -- true`, and `locus status --oneline`. Peers: `direnv exec <dir> true` after `direnv allow`, and `sops exec-env <file> true` with `SOPS_AGE_KEY_FILE` set. `sops --version` was not the latency command: it printed a warning that the command checks the network for a newer release.

**Scrub check.** The parent set `GH_TOKEN`, `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, and `DIRENV_BENCH` to synthetic values. A child script printed `set` or `unset` for each name and whether `HOME` matched the parent. Values are not recorded here.

**Which Locus binary.**

- npm `locus-cli@0.5.1` and `@ashlrai/locus-mcp@0.5.1` download the GitHub release. Tag `v0.5.1` is annotated object `dd6a4248ae58389074cebe5bb10db42a2eb56b20` and peels to commit `89f12eb3ab523f84ef5d03335054add4bc97314f`. The release was published 2026-10-26T02:00:55Z. Assets: `locus-aarch64-apple-darwin.tar.gz`, `locus-x86_64-apple-darwin.tar.gz`, `locus-x86_64-unknown-linux-gnu.tar.gz`, matching `.sha256` files, and `locus-adapters-v0.5.1.json`.
- `cargo install --git` of the success run built `e969d9c44a3c0bff1d0a1fee4deb1b77fb58652e` (main at that moment, after the tag). The failed 1.83.0 run fetched `1d534633`.
- Commit `357f98b` (README only, #161) landed after these binaries were built. It is not in the timed binaries.

`npm view locus-cli version` and `npm view @ashlrai/locus-mcp version` both printed `0.5.1` at 2026-10-26T02:08:37Z.

## Install to first useful result

| Path | Command | Wall | Exit | Tree peak RSS | First useful result |
|---|---|---|---|---|---|
| Locus npm | `npm install -g locus-cli@0.5.1 @ashlrai/locus-mcp@0.5.1` | 0.775 s | 0 | 182415360 bytes | quickstart in that shell, 0.123 s, stdout contained `whoami personal` |
| Locus cargo, default toolchain | `cargo install --git ... locus-cli locus-mcp --locked` with rustc 1.83.0 | 1.924 s | 101 | 160849920 bytes | no binary |
| Locus cargo, stable | same command, `RUSTUP_TOOLCHAIN=stable`, rustc 1.99.1, fresh cargo home | 86.488 s | 0 | 1485778944 bytes | `locus quickstart` on that binary, 0.138 s, exit 0, stdout contained `whoami personal` |
| direnv | [install.sh](https://direnv.net/install.sh) with `PATH` starting at a writable directory | 0.530 s | 0 | not sampled | `direnv allow` 3 ms, then `direnv exec` 19 ms, exit 0 |
| sops | `curl` of `sops-v3.13.3.linux.amd64` | 0.534 s | 0 | not sampled | see the age steps below |
| age | download and unpack v1.3.2 | 0.436 s | 0 | not sampled | `age-keygen`, `sops encrypt`, and `sops exec-env` together 0.040 s, exit 0 |

The npm figure 0.775 s is the Python timer (`TIMED_WALL_S` 0.775227). A shell stamp around that same install was 795760955 ns (0.796 s). npm printed `added 2 packages in 718ms`. From the install start stamp to both `--version` commands finishing was 1.428 s. Both printed `0.5.1`.

The 60 second target is met by the npm install plus quickstart. It is two commands. The cargo install on stable is one command and took 86.488 s before quickstart. The default-toolchain cargo command produced no binary. `brew` was not on this VM, so the README's Homebrew line was not run.

Cargo detail for the 86.488 s run (2026-10-26T02:12:28Z to 2026-10-26T02:13:55Z): `locus-cli` printed `Finished release` in 50.96 s, then `locus-mcp` compiled `locus-core` again and printed `Finished` in 1 m 26 s. Binaries: `locus` 12825264 bytes, `locus-mcp` 5493408 bytes.

The 1.924 s failure (`TIMED_WALL_S` 1.923873) stopped while parsing `zeroize_derive` 1.5.0. Cargo's text: feature `edition2024` is required, and that feature is not stabilized in Cargo 1.83.0. `cargo install --git` used the default toolchain. The repo file `rust-toolchain.toml` sets `channel = "stable"` and did not change that default, because the command's working directory was outside the repo.

direnv's installer wrote `/tmp/peer-bin/direnv` (8548514 bytes) from `https://github.com/direnv/direnv/releases/download/v2.38.2/direnv.linux-amd64`. `direnv version` printed `2.38.2`. The 0.530 s stamp delta is 529506322 ns. The v2.38.2 release has 24 assets.

sops `curl` wrote 52121762 bytes. The release notes' `mv` to `/usr/local/bin/sops` exited with `Permission denied` (`/usr/local/bin` is not writable here). The binary was marked executable in `/tmp/peer-bin`. `sops --version` printed `sops 3.13.3 (latest)` and the network-check warning. The age stamp delta is 435853763 ns. The keygen, encrypt, and exec-env stamp delta is 40064959 ns. Encrypt wrote 1030 bytes and did not open an editor. exec-env printed the decrypted value.

npm release binary sizes: `locus` 10011528 bytes, `locus-mcp` 4321256 bytes, both stripped ELF files. The nvm `locus` path is a symlink chain that ends at that ELF.

`npx --yes locus-cli@0.5.1 --help`, with the nvm prefix already holding the global install and a fresh npm cache, exited 0 in 0.470 s and printed help. stderr was the npm upgrade notice.

## Core-operation latency

HEAD binary `e969d9c4`, pin live, 20 runs after one warmup, every exit 0. direnv used `HOME` that already held the allow file. An earlier direnv series exited 1 on every run with `.envrc is blocked` because `HOME` did not match that allow file. Those exit 1 times are not in the table.

| Command | Min | Median | Max | `ru_maxrss` min / median / max (KiB) |
|---|---|---|---|---|
| `locus whoami` | 0.020 s | 0.020 s | 0.021 s | 13400 / 13528 / 13528 |
| `locus exec -- true` | 0.050 s | 0.051 s | 0.051 s | 13528 / 13528 / 13528 |
| `locus status --oneline` | 0.020 s | 0.020 s | 0.021 s | 13528 / 13528 / 13528 |
| `direnv exec <dir> true` | 0.015 s | 0.016 s | 0.016 s | 13648 / 13648 / 13648 |
| `sops exec-env <file> true` | 0.010 s | 0.011 s | 0.014 s | 34064 / 34448 / 35472 |

Recorded medians before rounding to 0.001 s: locus whoami 0.020236 s, locus exec 0.050681 s, locus status 0.020176 s, direnv 0.015753 s, sops 0.011385 s. Recorded mins and maxes: whoami 0.019733 to 0.020529 s, exec 0.049903 to 0.051272 s, status 0.019644 to 0.020645 s, direnv 0.014731 to 0.016409 s, sops 0.009991 to 0.013710 s.

## Ambient environment

| Launcher | Four parent variables | `HOME` | Exit |
|---|---|---|---|
| plain Python | set | same | 0 |
| `locus exec` (HEAD, live pin) | unset | rewritten | 0 |
| `direnv exec` after allow | set | same | 0 |
| `sops exec-env` | set | same | 0 |
| `sops exec-env --pristine` | unset | same | 0 |

The locus line's stderr included `scrubbed 41 ambient` and `resolved 0 secret(s)`, then three warnings that the sample providers' injection variables were unset (`SUPABASE_ACCESS_TOKEN`, `GH_TOKEN,GITHUB_TOKEN,GITHUB_PERSONAL_ACCESS_TOKEN`, `VERCEL_TOKEN`). Exit stayed 0. Other locus exec runs on this VM printed `scrubbed` 38 or 39 when the parent environment differed. The controlled canary run is the 41.

`locus exec --expect acme` while `personal` was pinned exited 1. stderr: `pin_mismatch: expected binding 'acme' but 'personal' is pinned; nothing was run. Run: locus enter acme`.

## Feature notes from these runs

Locus, on this machine: a sealed pin, `--expect` refusing before the child starts, a default scrub of the four canaries plus a rewritten `HOME`, and an MCP catalog that grows only after `locus mcp --supervised` (9 tools with no pin, 10 for a direct `locus-mcp` while a pin exists, 18 with the supervisor). `locus_request_pin` is the pin tool agents get. Descriptions are prefixed with `[locus:unpinned]` or `[locus:personal]`, so the text changes when the alias changes. `inputSchema` on every listed tool had `additionalProperties: false`. Two `tools/list` results in one process matched, and two processes matched, comparing `result` only (the JSON-RPC `id` differs). Unknown tool `no_such_tool` returned JSON-RPC error `-32602`.

direnv, on this machine: a 0.530 s binary install, a shell hook the installer tells you to add, and a faster `exec` (median 0.016 s against locus exec 0.051 s) at a similar RSS (13648 KiB against 13528 KiB). The four canaries and `HOME` were still the parent's. The v2.38.2 release has 24 assets. Directory-change latency of the shell hook was not timed. Locus documents opt-in `LOCUS_AUTO_ENTER=1` in `docs/firm-mode.md` and the shell hook source. That path was not timed.

sops, on this machine: an encrypted dotenv (1030 bytes) and `exec-env` median 0.011 s. Default exec-env kept the four canaries and the parent `HOME`. `--pristine` cleared the four canaries and left `HOME` the same. The binary is 52121762 bytes. The release asset used here is `sops-v3.13.3.linux.amd64`.

Locus prebuilt assets on the v0.5.1 release are the three platform archives listed above. This VM is `x86_64-unknown-linux-gnu`, which has an asset. Linux arm64 and Windows are not in that asset list.
