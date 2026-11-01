# `GRAPH_HOME` lifecycle in the E2E suite

[`scripts/e2e.sh`](../scripts/e2e.sh) uses `GRAPH_HOME` only for the
fresh-home fallback in its optional graph export/import roundtrip. It is a
script-owned temporary directory, not a setting for choosing a Locus home.

1. **Initial state.** After creating the main `LOCUS_HOME`, the script clears
   `GRAPH_HOME` before installing its `EXIT` trap. An inherited value is not
   used or removed. The cleanup function tolerates an empty value.
2. **Creation.** In section 16, the script first checks for graph support,
   exports a graph into the main home, and attempts to import it there, first
   normally and then with `--force`. Only if both imports fail does it assign
   `GRAPH_HOME` a new directory from `mktemp -d`, using the template
   `locus-graph-imp.XXXXXX` directly under a nonempty `TMPDIR`, or `/tmp` if
   `TMPDIR` is unset or empty. A supplied `TMPDIR` must exist and be writable.
   No graph home is created when the graph section is skipped or either
   same-home import succeeds.
3. **Use.** The fallback runs `locus init` and then `locus graph import` with
   `LOCUS_HOME` set to `GRAPH_HOME` for each command. The import reads the
   export from the main home and receives the same graph passphrase. Either
   command failing stops the suite rather than skipping the fallback.
4. **Cleanup.** After a successful fallback import, `rm -rf` immediately
   removes the graph home. The variable remains set, so the suite's `EXIT`
   trap attempts to remove it again; the trap also removes it if initialization
   or import fails after creation. The main home and export are removed by
   the same trap. The trap runs on normal shell exit, including ordinary
   errors, but cannot run after an uncatchable termination such as `SIGKILL`;
   removal errors can also leave a directory behind. Inspect any leftover
   `locus-graph-imp.*` directory before removing it, and preserve its parent
   and unrelated files.

The suite also checks creation and exit cleanup unconditionally in section 2d,
using a separate temporary parent and both successful and failing exits. It
checks that unrelated files survive. The focused graph test runs the actual
section with simulated import outcomes and verifies that the lifecycle probe
detects a missing cleanup:
`node --test scripts/e2e-graph-isolation.test.mjs`. The temporary resource
test, `node --test scripts/e2e-temp.test.mjs`, checks that an inherited
`GRAPH_HOME` survives early exit. The graph fallback tests in
[`scripts/e2e.test.mjs`](../scripts/e2e.test.mjs) exercise the real release CLI
and cleanup on success and failure.
