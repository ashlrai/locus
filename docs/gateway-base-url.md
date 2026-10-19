# Gateway base URL for `locus exec` (opt-in)

`locus exec` rebuilds the child environment from a runtime allowlist, so a base URL such as `OPENAI_BASE_URL` exported in the parent shell never reaches the agent (see [gateway-base-urls.md](gateway-base-urls.md)). The `[gateway]` block lets you name a model gateway in config instead. It is **off by default**. With the block absent, or `enabled` anything other than `true`, the child environment is exactly what it was before.

```toml
# $LOCUS_HOME/config.toml (default ~/.locus/config.toml)
[gateway]
enabled = true
base_url = "http://127.0.0.1:4000/v1"
child_env = ["OPENAI_BASE_URL"]
# Required for any host other than 127.0.0.1 or [::1]:
# allowed_hosts = ["gw.example.com:443"]
```

## What the child gets
- Each variable listed in `child_env` is set to `base_url`. Nothing else is added.
- The value comes from config only. The parent shell never selects a target, and a parent `OPENAI_BASE_URL` is still dropped and reported as scrubbed.
- Only the URL is passed. Locus never forwards a gateway key, token or header. Keep the provider keys on the gateway side; Locus still injects only the binding's own credential variables.
- A listed variable that Locus already sets is not overwritten.
- It applies to the `locus exec` child environment only, not to `locus ci env` exports.

## Rules
These match phantom's governed-client gateway validator (`src/core/run/gateway-target.ts`).
- Schemes: `https` always, `http` only for loopback.
- Loopback means the IP literals `127.0.0.1` and `[::1]` only. `localhost` and `*.localhost` are refused, as is every other IP literal (for example `10.0.0.5`).
- A non-loopback host needs an exact `host:port` entry in `allowed_hosts`, including the port (`gw.example.com:443`). There are no wildcards and no suffix matching.
- Refused: userinfo (`user:pass@`), a query, a fragment, non-canonical hosts (trailing dot, numeric shorthands such as `0x7f.1`, percent-encoding, non-ASCII), dot or empty path segments, backslashes, percent-encoding and whitespace.
- `child_env` names must be upper-case and end in `_BASE_URL` or `_API_BASE`. Names that contain `KEY`, `TOKEN`, `SECRET`, `PASSWORD`, `CREDENTIAL` or `AUTH`, or start with `LOCUS_`, are refused. At most 8 names.

## When the block is refused
If `enabled = true` but the block fails a rule, Locus fails closed. The run gets no gateway URL **and no credentials** (none are resolved), so a real key never reaches an agent that would talk to the provider directly instead of the gateway you configured. The failure is reported as a credential issue with `source=gateway` and a closed code such as `loopback-name-not-allowed`, `host-not-allowlisted` or `child-env-invalid`. The code never contains the URL. Under strict credential mode the run stops.

A config file that does not parse at all (for example `allowed_hosts` given as a string) falls back to defaults, which means the gateway is off.
