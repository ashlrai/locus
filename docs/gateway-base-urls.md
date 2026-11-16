# Model gateways and base URLs (audit note)

> Update: an opt-in `[gateway]` config block now passes a validated base URL to the `locus exec` child. See [gateway-base-url.md](gateway-base-url.md). The default behaviour described below is unchanged.

This note describes the default environment and options for pointing an agent at a model gateway, such as a local LiteLLM proxy or a Phantom route.

## Findings
- **Locus makes no model-provider calls.** The opt-in gateway and inherited-environment settings can pass validated base URLs to an exec child. Its HTTP surfaces are its own loopback servers (`locus serve` on 127.0.0.1, the MCP HTTP mode).
- **Locus injects credentials, not endpoints.** `inject_keys_for_provider` (`crates/locus-core/src/credential.rs`) maps a provider to the key environment variable that receives the resolved secret, for example `OPENAI_API_KEY` and `ANTHROPIC_API_KEY`. The separate opt-in gateway configuration can set base URL variables.
- **`locus exec` drops unknown variables.** `crates/locus-core/src/isolation.rs` rebuilds the child environment from a small runtime allowlist (`PATH`, `LANG`, `TERM`, `COLORTERM`, `NO_COLOR`, `TMPDIR`, `TMP`, `TEMP`, Windows basics and `LC_*`). Its comment states that identity, provider, cloud, proxy and application variables are excluded. A variable such as `OPENAI_BASE_URL`, `ANTHROPIC_BASE_URL` or `HTTPS_PROXY` exported in the parent shell therefore does not reach an agent started through `locus exec`.
- The audit webhook is separate from the child gateway settings.

## What this means for a gateway
By default, exporting a base URL in the parent shell is not enough. Configure the validated `[gateway]` block, opt in to exact parent variables with `[exec] inherit_env` as described in [PHANTOM-USAGE.md](PHANTOM-USAGE.md), or set a URL inside the isolated child as shown below.

## Setting a gateway URL inside `locus exec`
Without either opt-in, set the URL on the command that `locus exec` runs, not in the parent shell. `PATH` is on the allowlist, so the standard `env` tool is available to the child:

```sh
# Wrong: dropped by locus exec, the agent never sees it
export OPENAI_BASE_URL=http://127.0.0.1:4000
locus exec -- my-agent

# Right: set inside the isolated child
locus exec -- env OPENAI_BASE_URL=http://127.0.0.1:4000 my-agent
```

Or put the variable in a small launcher script that `locus exec` runs:

```sh
locus exec -- ./run-agent.sh   # run-agent.sh exports OPENAI_BASE_URL, then starts the agent
```

Notes:
- Use a loopback or otherwise trusted gateway URL. Keep real provider keys on the gateway side; Locus still injects only its own key variables into the child.
- Proxy variables (`HTTPS_PROXY` and similar) follow the same rule and must be set the same way.
- This is based on reading `isolation.rs`; the examples were not run against a built binary for this note.
- Both gateway and inherited-environment settings are opt-in and validated.
