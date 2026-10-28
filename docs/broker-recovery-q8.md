# Recovering a pin after its broker exits

A pin's authority broker lives with the operator process that created it. If
that shell exits or crashes, the saved session can remain while its broker is
unavailable. Commands may fail with `no current broker endpoint` or `human
re-pin required`. They fail closed; the saved alias is not proof of authority.
A human operator must recover the pin in an operator shell, using the same
`LOCUS_HOME` as the affected session and the appropriate control capability.
Agents cannot re-pin through MCP.

## Enter first

In a new operator shell, load the control capability with the hook for that
shell, then enter the intended binding. For Bash:

```bash
eval "$(locus hook bash)"
locus enter <alias>
locus whoami
```

Replace `<alias>` with an existing, intended binding alias. Use `locus binding
list` to find aliases if needed. For zsh use `eval "$(locus hook zsh)"`; for
fish use `locus hook fish | source`. The hook relies on the operator's control
capability being available; a missing or mismatched capability must be resolved
by the operator, not bypassed by an agent.

For an intact local-control session whose broker is unavailable or whose
broker generation is stale, `enter` can replace the active pin directly after
authenticating the control capability. An authentic expired local-control pin
can also be replaced. Replacement is audited as `session.replace_stale`.
`whoami` checks the resulting identity; keep the operator shell alive while
using its broker. Normal binding and workspace allowlist checks still apply.
An explicitly selected `LOCUS_SESSION_ID` uses the strict authority check and
does not take the active-pin replacement path.

## If enter is refused

Read the error before taking further action. A bad or legacy seal or an
unreadable active session cannot be replaced by `enter`. When the active
session is wedged and teardown is appropriate, a human can run:

```bash
locus leave --force
locus enter <alias>
locus whoami
```

`leave --force` clears the active session and records `session.force_leave`;
it does not mint a new pin. It requires the control capability and verifies it
against a live broker or the persisted operator capability. Do not use forced
leave to get around a wrong capability, a workspace refusal, or another
ordinary `enter` error. Do not manually delete the session file.

For the exceptional case where neither a broker nor a persisted capability is
available, the CLI offers `locus leave --force --no-verifier` as an explicit
operator acknowledgement of unverified teardown. It does not override a live
broker's refusal or a mismatching persisted capability; a subsequent `enter`
still requires the normal authorization checks.

Recovery hints may prescribe force-first for an expired pin, although the
current replacement gate accepts an authentic expired local-control session.
Treat a hint as guidance for the diagnosed error and stop if an authorization
check refuses the operation. See [policy](policy.md) and
[onboarding](onboarding.md) for control capability and workspace details.
