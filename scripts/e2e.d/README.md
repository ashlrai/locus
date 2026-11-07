# e2e drop-in steps

`scripts/e2e.sh` sources every `scripts/e2e.d/*.sh` here, in `LC_ALL=C` sorted
order, after its own steps and before the final PASS summary.

**New end-to-end steps go in a new file here**, not in `scripts/e2e.sh`. Many
PRs edit that one file at once, so appending to it collides; a new file never
does.

- Name it `NN-topic.sh` (e.g. `20-widget-pin.sh`) so order is explicit.
- Steps are sourced into the e2e shell: use `log`, `ok`, `skip`, `die`, `locus`,
  `$LOCUS_BIN`, `$MCP_BIN`, `$LOCUS_HOME`, `$ROOT`. Fail with `die "msg"`; any
  `exit`/failing command under `set -e` also fails the whole run.
- No shebang needed; files are sourced, not executed.
- Use only the isolated `$LOCUS_HOME`; never touch `~/.locus`, never print secrets.
- Register temp dirs for cleanup yourself (`trap` is owned by `e2e.sh`; put paths
  under `$LOCUS_HOME`, which is removed on exit).
- Other files (like this README) are ignored; only `*.sh` runs.

Prove the mechanism with `bash scripts/check-e2e-dropin.sh`.
