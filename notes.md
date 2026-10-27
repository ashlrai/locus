# Peer CLI Notes

- Initial worktree has a pre-existing modification to .github/PULL_REQUEST_TEMPLATE.md. Do not touch it.
- No publish, commit or push permitted by final user instructions.
- Current HEAD ae2c14c has no peer registry or CLI.
- Local codex-bridge/RQ-PARITY-LOCUS-PEERS also points to ae2c14c; its worktree contains no implementation changes.
- Local codex-bridge/RQ-PLAN-LOCUS-PEERS-A at e1ce854 contains a predecessor registry with GET/POST /peers and register/list/serve commands only. It has no send, inbox, thread, or message schema.
- Requested upstream contract clarification asynchronously. Do not invent messaging routes or import unrelated branch changes.
