# Sample exec without unset-credential warnings

This local smoke uses the generated `personal` binding without credentials. It
makes no provider API calls.
Run the entire block in one Bash shell: the pin's authority broker lives only
as long as the shell that creates it. Build `locus` first and put it on PATH.

```bash
set -e
export LOCUS_HOME="$(mktemp -d)"
locus init --with-samples
eval "$(locus hook bash)"
locus pin personal
locus whoami
locus exec --no-resolve --expect personal -- sh -c 'test "$LOCUS_BINDING" = personal && echo "sample exec succeeded"'
```

The last command should print `sample exec succeeded`, without warnings about
unset credential variables. `--no-resolve` leaves the sample binding's `env:`
references unresolved. For provider work, omit that flag, supply credentials
outside the agent context, and replace the sample scope placeholders. Ordinary
exec warns when a credential is missing; `--strict-creds` fails instead.
