# Sample exec without unset-credential warnings

This local smoke uses the generated `personal` binding without resolving
credentials or making provider API calls.
Run the entire block in one Bash shell: the pin's authority broker lives only
as long as the shell that creates it. Build `locus` first and put it on PATH.

```bash
set -e
export LOCUS_HOME="$(mktemp -d)"
locus init --with-samples
eval "$(locus hook bash)"
locus pin personal
locus whoami
locus exec --expect personal --no-resolve -- sh -c 'test "$LOCUS_BINDING" = personal && echo "sample exec succeeded"'
```

The last command should print `sample exec succeeded`, without warnings about
unset credential variables. `--no-resolve` skips the sample binding’s credential
references and refuses credential-resolving upstreams. For provider work, omit
that flag, supply credentials outside agent context, and replace the sample scope
placeholders.
