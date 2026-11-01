# E2E graph import isolation

The [graph export/import roundtrip in section 16](../scripts/e2e.sh#L1273)
of `scripts/e2e.sh` is feature-detected. If graph support is unavailable, or
an export cannot be produced, the section reports a skip. A successful export
is stored in the suite's main `LOCUS_HOME` and checked for the `LOCUSGRAPH`
magic before import is attempted.

The script first tries importing into that same home, then retries with
`--force`. A successful same-home import ends the roundtrip without creating a
separate graph home; success may mean existing bindings were skipped. Only if
both attempts fail does the script create `GRAPH_HOME` with `mktemp -d`, using
the `locus-graph-imp.XXXXXX` template directly under nonempty `TMPDIR`, or
under `/tmp` when `TMPDIR` is unset or empty. A supplied `TMPDIR` must already
exist and be writable.

The fallback runs `locus init` and then imports the original export with
`LOCUS_HOME` set to `GRAPH_HOME` for each command. Both commands receive the
same graph passphrase used for the export. This exercises a fresh-home write
path without importing into the main test home. If initialization or import
fails, the suite fails; the fallback does not silently skip the error.

After a successful fallback import, the script immediately removes `GRAPH_HOME`.
The [suite's EXIT cleanup trap](../scripts/e2e.sh#L68) also removes it, including
on ordinary failure after creation, along with the other suite-owned temporary
resources. The trap cannot cover termination that prevents it from running, and
removal errors can leave files behind. Inspect any leftover `locus-graph-imp.*`
directory before removing it; do not remove the parent temporary directory or
unrelated files.

The focused test runs the actual graph section with simulated CLI outcomes:
`node --test scripts/e2e-graph-isolation.test.mjs`. The graph fallback cases
in [`scripts/e2e.test.mjs`](../scripts/e2e.test.mjs) additionally force the
same-home imports to fail and check fresh-home isolation and cleanup against
the release CLI. Run them with `node --test scripts/e2e.test.mjs`; the file
also exercises other end-to-end regressions.
