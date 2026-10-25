# Using Locus with Phantom

This page covers running Locus next to Phantom Secrets (`phantom exec`, `phm_` placeholders, `phm:` references) and a model gateway such as LiteLLM. For enrolled-repo bindings see [phantom-enrolled-repos.md](phantom-enrolled-repos.md).

Locus itself makes no model-provider calls, so it has no API key or base URL of its own to send through Phantom. What it controls is the environment of the agent that `locus exec` starts.

## Running under `phantom exec`

`phantom exec -- <cmd>` does three things:
- It starts `<cmd>` with fresh `phm_` session placeholders instead of real provider keys.
- It points the provider base URLs (for example `OPENAI_BASE_URL` and `ANTHROPIC_BASE_URL`) at a local Phantom proxy: `http://127.0.0.1:<port>/<route>/_phantom/<token>/`.
- On the way out, the proxy swaps each placeholder for the real key, so the real key never enters the process.

What changes with Locus:
- **`locus exec` drops those variables by default.** It rebuilds the child environment from a small runtime allowlist ([gateway-base-urls.md](gateway-base-urls.md)). Unless you opt in with `[exec] inherit_env` (below), the `phm_` placeholders and proxy base URLs that `phantom exec` puts into Locus's own environment do not reach the agent.
- **Base URLs go through the `[gateway]` block.** This opt-in block in `$LOCUS_HOME/config.toml` sets the `*_BASE_URL` variables you list in `child_env` to one validated `base_url` ([gateway-base-url.md](gateway-base-url.md)). The rules: `https`, or plain `http` only to `127.0.0.1` or `[::1]`; a non-loopback host needs an exact `host:port` in `allowed_hosts`; Locus never forwards a gateway key, token or header.
- **Don't put the Phantom proxy URL in `base_url`.** It carries a per-session token, so it changes every session, and it would be written to disk. Point `base_url` at a stable gateway, such as a local LiteLLM proxy, instead.
- **Running it end to end is untested.** The variable handling below is covered by tests, but `phantom exec -- locus exec -- <agent>` has not been run against a live Phantom session.

## Passing `phantom exec` variables through (`[exec] inherit_env`, opt-in)

`[exec] inherit_env` lists exact parent variable names that the `locus exec` child may inherit. It is empty by default, and empty means the child environment is exactly what it was before.

```toml
# $LOCUS_HOME/config.toml (default ~/.locus/config.toml)
[exec]
inherit_env = ["ANTHROPIC_API_KEY", "ANTHROPIC_BASE_URL", "PHANTOM_PROXY_PORT"]
```

```sh
phantom exec -- locus exec -- my-agent
```

Rules (`crates/locus-core/src/inherit_env.rs`):
- **Exact names only.** A name must match `[A-Za-z_][A-Za-z0-9_]*`, at most 64 bytes, at most 32 names. Globs and patterns (`ANTHROPIC_*`) are refused. Names starting with `LOCUS_`, and `HOME`, `USERPROFILE`, `GH_CONFIG_DIR`, `AWS_CONFIG_FILE` and `AWS_SHARED_CREDENTIALS_FILE`, are refused too. An invalid list inherits nothing.
- **Key-shaped names cross only as `phm_` placeholders.** A name containing `KEY`, `TOKEN`, `SECRET`, `PASSWORD`, `PASSWD`, `CREDENTIAL`, `AUTH`, `PRIVATE` or `COOKIE` (any case) is inherited only when its value is a `phm_` placeholder. A real key under such a name is refused.
- **Other names are checked too, as a best effort.** A value that starts like a well-known key (for example `sk-`, `ghp_`, `github_pat_`, `xoxb-` or `AKIA`) is refused, and so is a URL with a user name or password. This is not a complete secret scanner, so list only variables you know.
- **Base URLs follow the `[gateway]` rules.** A `*_BASE_URL` or `*_API_BASE` value must be `https`, or `http` only to `127.0.0.1` or `[::1]`. Any other host needs an exact `host:port` in `[gateway] allowed_hosts`. The Phantom proxy URL is loopback http, so it passes.
- **Locus's own values win.** An inherited value never overwrites a variable Locus sets, the `[gateway]` URL, or a credential injected from the binding.
- **Refusals carry no names or values.** A refused name is reported as a credential issue with `source=inherit_env` and a closed code (`inherit-env-invalid`, `inherit-env-secret-refused`, `inherit-env-base-url-refused`, `inherit-env-userinfo-refused`, `inherit-env-value-refused`). Under strict credential mode the run stops.
- It applies to the `locus exec` child environment only, not to `locus ci env` exports.

Because the Phantom proxy URL changes every session, this route passes it through from the `phantom exec` session instead of writing it into `[gateway] base_url`.

## `phm:` credential references

A binding can name a Phantom secret with `--credential-ref phm:NAME`. `LOCUS_PHANTOM_PROJECT`, when set, names the Phantom vault directory used for every `phm:` reference.

Current status: `phm:` references do not resolve with Phantom Secrets 0.7.9 ([phantom-enrolled-repos.md](phantom-enrolled-repos.md)). The run starts with the provider variables unset, `--strict-creds` fails before the child runs, and `locus doctor` reports `WARN` with `[unresolved_phm]`. No new resolver is planned: getting a plaintext value out of Phantom is exactly what this setup avoids. For model keys, use `phantom exec` placeholders with `[exec] inherit_env`. For provider bindings, use `env:` references and supply the value from your own environment, outside agent context.

Do not use `phantom reveal` to feed a key to Locus or to an agent, and do not capture its output.

## LiteLLM

If a LiteLLM proxy sits between the agent and the providers, use LiteLLM 1.83.0 or newer, never 1.82.7 or 1.82.8.

- **Why 1.83.0:** `litellm` 1.82.7 and 1.82.8 on PyPI were compromised releases that carried credential-stealing code (GHSA-5mg7-485q-xm76, March 2026). 1.83.0, published to PyPI on 2026-03-31, was the first clean release, built by LiteLLM's rebuilt release pipeline ([LiteLLM security update](https://docs.litellm.ai/blog/security-update-march-2026)).
- **Use a current release, not just 1.83.0.** Later advisories were fixed in later releases (for example GHSA-3cv6-jpf6-8222). On 2026-10-19 the current PyPI release was 1.106.0. Check LiteLLM's security advisories before you pick or upgrade a version.
- **Install it verifiably.** Use a hash-pinned install (`pip install --require-hashes -r requirements.txt`) or the official `ghcr.io/berriai/litellm` image, checked with `cosign verify` as LiteLLM's README shows. Run it in a container that does not mount your home directory or dotfiles.
- Keep the provider keys on the LiteLLM side. Give the agent only the proxy URL and, if the proxy needs one, a proxy key that is separate from the provider keys.

For Locus, the LiteLLM proxy is what `[gateway] base_url` points at, for example `http://127.0.0.1:4000/v1` with `child_env = ["OPENAI_BASE_URL"]`.

## What is tested

The `[gateway]` rules and the child environment are covered by tests in `crates/locus-core/tests/gateway_base_url.rs`, and `[exec] inherit_env` by `crates/locus-core/tests/inherit_env.rs` and the unit tests in `inherit_env.rs`. The default-path pin in `crates/locus-core/tests/isolation.rs` is unchanged. This page was written by reading the source and the linked docs. The `phantom exec` combinations above and a live LiteLLM proxy were not run for it.
