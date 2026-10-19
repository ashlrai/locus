# Using Locus with Phantom

This page covers running Locus next to Phantom Secrets (`phantom exec`, `phm_` placeholders, `phm:` references) and a model gateway such as LiteLLM. For enrolled-repo bindings see [phantom-enrolled-repos.md](phantom-enrolled-repos.md).

Locus itself makes no model-provider calls, so it has no API key or base URL of its own to send through Phantom. What it controls is the environment of the agent that `locus exec` starts.

## Running under `phantom exec`

`phantom exec -- <cmd>` does three things:
- It starts `<cmd>` with fresh `phm_` session placeholders instead of real provider keys.
- It points the provider base URLs (for example `OPENAI_BASE_URL` and `ANTHROPIC_BASE_URL`) at a local Phantom proxy: `http://127.0.0.1:<port>/<route>/_phantom/<token>/`.
- On the way out, the proxy swaps each placeholder for the real key, so the real key never enters the process.

What changes with Locus:
- **`locus exec` drops those variables.** It rebuilds the child environment from a small runtime allowlist ([gateway-base-urls.md](gateway-base-urls.md)). The `phm_` placeholders and proxy base URLs that `phantom exec` puts into Locus's own environment do not reach the agent. So `phantom exec -- locus exec -- my-agent` does not hand the agent the proxy.
- **Base URLs go through the `[gateway]` block.** This opt-in block in `$LOCUS_HOME/config.toml` sets the `*_BASE_URL` variables you list in `child_env` to one validated `base_url` ([gateway-base-url.md](gateway-base-url.md)). The rules: `https`, or plain `http` only to `127.0.0.1` or `[::1]`; a non-loopback host needs an exact `host:port` in `allowed_hosts`; Locus never forwards a gateway key, token or header.
- **Don't put the Phantom proxy URL in `base_url`.** It carries a per-session token, so it changes every session, and it would be written to disk. Point `base_url` at a stable gateway, such as a local LiteLLM proxy, instead.
- **Combining is untested.** Running `phantom exec` and `locus exec` in one command line, in either order, has not been tested and is not documented as supported here.

## `phm:` credential references

A binding can name a Phantom secret with `--credential-ref phm:NAME`. `LOCUS_PHANTOM_PROJECT`, when set, names the Phantom vault directory used for every `phm:` reference.

Current status: `phm:` references do not resolve with Phantom Secrets 0.7.9 ([phantom-enrolled-repos.md](phantom-enrolled-repos.md)). The run starts with the provider variables unset, `--strict-creds` fails before the child runs, and `locus doctor` reports `WARN` with `[unresolved_phm]`. Until a reviewed Phantom and Locus handoff ships, use `env:` references and supply the value from your own environment, outside agent context.

Do not use `phantom reveal` to feed a key to Locus or to an agent, and do not capture its output.

## LiteLLM

If a LiteLLM proxy sits between the agent and the providers, use LiteLLM 1.83.0 or newer, never 1.82.7 or 1.82.8.

- **Why 1.83.0:** `litellm` 1.82.7 and 1.82.8 on PyPI were compromised releases that carried credential-stealing code (GHSA-5mg7-485q-xm76, March 2026). 1.83.0, published to PyPI on 2026-03-31, was the first clean release, built by LiteLLM's rebuilt release pipeline ([LiteLLM security update](https://docs.litellm.ai/blog/security-update-march-2026)).
- **Use a current release, not just 1.83.0.** Later advisories were fixed in later releases (for example GHSA-3cv6-jpf6-8222). On 2026-10-19 the current PyPI release was 1.106.0. Check LiteLLM's security advisories before you pick or upgrade a version.
- **Install it verifiably.** Use a hash-pinned install (`pip install --require-hashes -r requirements.txt`) or the official `ghcr.io/berriai/litellm` image, checked with `cosign verify` as LiteLLM's README shows. Run it in a container that does not mount your home directory or dotfiles.
- Keep the provider keys on the LiteLLM side. Give the agent only the proxy URL and, if the proxy needs one, a proxy key that is separate from the provider keys.

For Locus, the LiteLLM proxy is what `[gateway] base_url` points at, for example `http://127.0.0.1:4000/v1` with `child_env = ["OPENAI_BASE_URL"]`.

## What is tested

The `[gateway]` rules and the child environment are covered by tests in `crates/locus-core/tests/gateway_base_url.rs`. This page was written by reading the source and the linked docs. The `phantom exec` combinations above and a live LiteLLM proxy were not run for it.
