# Building both executables

`locus-cli` contains the `locus` and `locus-mcp` binary targets. Both use the
same `locus-core` in a single Cargo build; the MCP target calls the same library
entry point as the standalone `locus-mcp` package. Install both with
`cargo install --locked --path crates/locus-cli` from a checkout, or use the
single-package Git command in the README. The independent MCP package remains
available for server-only installations.

To check a clean build and produce Cargo's HTML timing report:

```bash
build_dir=$(mktemp -d)
CARGO_TARGET_DIR="$build_dir" cargo build --locked -p locus-cli --timings \
  --message-format=json > "$build_dir/artifacts.jsonl" 2> "$build_dir/build.log"
```

Inspect `build.log` for one `Compiling locus-core` line and the timing report
location. The JSON stream records the core library artifact and both executable
targets; the timing report is under `$build_dir/cargo-timings/`. A fresh target
is necessary to distinguish compilation from a cached build. Timings are
machine-dependent and are not a performance guarantee. This measures the
checkout build, not a published package or a two-package `cargo install`.

The lockfile adds the CLI-to-MCP workspace dependency edge, but its set of
package names, versions, and sources is unchanged.

On 2026-10-31, a fresh debug build on macOS arm64 with Cargo 1.95.0 and
rustc 1.95.0 completed in 17m 43s while other checks were running. Cargo wrote
its HTML timing report, logged one `Compiling locus-core` line, and emitted one
non-fresh `locus_core` compiler artifact plus both executable artifacts. The
machine and concurrent workload make this an observation, not a speed target.
