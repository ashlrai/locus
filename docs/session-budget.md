# Session budget caps (opt-in)

Stop an agent session after a number of tool calls or a number of minutes. Off by default: with no `[budget]` section in `$LOCUS_HOME/config.toml`, nothing changes and nothing is written to disk.

```toml
[budget]
max_actions = 200   # tool calls per session
max_minutes = 90    # minutes from the session's first counted call
```

Either key can be used alone. A value of `0` is treated as unset (a zero budget would silently lock every session out; remove the key instead).

## What is counted

- Every MCP `tools/call` for a session that reaches the policy gate. Calls that policy denies or holds for approval count too, because a looping agent is what a cap is for.
- Not counted: human CLI commands, and tool calls dispatched without a session id.
- The minutes window starts at the session's first counted call. Use `max_ttl` in a binding policy for a wall-clock expiry from pin time.

## What happens at the cap

- The call is refused with `{"error": "session_budget_exhausted", "detail": ..., "hint": ...}`. Nothing runs and no approval record is created.
- The stop is sticky for that session, even if the cap is later raised. The session keeps its pin and seal. A human lifts the stop by pinning a new session (new sessions start a fresh count).
- One `budget.exhausted` entry is written to the audit log per stopped session.
- Nothing is applied or undone automatically: no unpinning, no rollback, no credential change. The cap only refuses further calls.
- If a cap is configured but the counter cannot be read or written, the call is refused (fail closed).

## State

Counters live in `$LOCUS_HOME/budgets/<session_id>.json` (`actions`, `started_at`, and `stopped` once tripped). They hold no credentials. Delete the file to reset one session.

## Relationship to other limits

This is independent of the seal, scope freeze, approvals and `parallel_sessions`; it adds a refusal and never grants anything. Credential handling is unchanged.
