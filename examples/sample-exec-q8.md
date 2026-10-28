# Sample exec without unset-credential warnings

This local smoke uses the generated `personal` binding and synthetic values. It
makes no provider API calls; these values cannot authenticate to a provider.
Run the entire block in one Bash shell: the pin's authority broker lives only
as long as the shell that creates it. Build `locus` first and put it on PATH.

```bash
set -e
export LOCUS_HOME="$(mktemp -d)"
export LOCUS_SUPABASE_PERSONAL=sample-supabase-token
export LOCUS_GH_TOKEN_PERSONAL=sample-github-token
export LOCUS_VERCEL_TOKEN_PERSONAL=sample-vercel-token
locus init --with-samples
eval "$(locus hook bash)"
locus pin personal
locus whoami
locus exec --expect personal -- sh -c 'test "$LOCUS_BINDING" = personal && echo "sample exec succeeded"'
```

The last command should print `sample exec succeeded`, without warnings about
unset credential variables. The three `env:` references in the sample binding
resolve from the parent shell; their values are deliberately not printed. Use
real credentials only when performing real provider work, supplied outside the
agent context, and replace the sample scope placeholders before that work.
