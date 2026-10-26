//! Native CLI routing is a credential boundary for trusted executables, like
//! ordinary exec. It does not translate arbitrary CLI commands into MCP tools.

use super::{adapter_for, known_providers, ProviderAdapter};
use crate::binding::Binding;
use crate::error::{LocusError, Result};
use crate::isolation::IsolatedEnv;
use std::path::Path;

pub struct CliRoute {
    adapter: Box<dyn ProviderAdapter>,
    binding: Binding,
}

impl CliRoute {
    /// Select exactly one provider from the already verified pinned binding.
    /// Never guess between multiple accounts of the same provider.
    pub fn resolve(binding: &Binding, cli: &str, program: &str) -> Result<Self> {
        let adapter = known_providers()
            .iter()
            .filter_map(|name| adapter_for(name))
            .find(|adapter| adapter.cli_name() == Some(cli))
            .ok_or_else(|| {
                LocusError::msg("unsupported CLI for --for; use gh, vercel, supabase, or aws")
            })?;
        // An explicit executable path is supported, but a shell or a different
        // CLI cannot silently receive the selected provider's credentials.
        if Path::new(program).file_name().and_then(|s| s.to_str()) != Some(cli) {
            return Err(LocusError::msg(
                "--for CLI must match the command executable name",
            ));
        }
        let mut providers = binding
            .providers
            .iter()
            .filter(|p| p.provider.eq_ignore_ascii_case(adapter.name()));
        let provider = providers.next().ok_or_else(|| {
            LocusError::msg(format!(
                "CLI {cli} is not pinned: the active binding has no {} provider",
                adapter.name()
            ))
        })?;
        if providers.next().is_some() {
            return Err(LocusError::msg(format!(
                "ambiguous CLI route: the active binding has multiple {} providers",
                adapter.name()
            )));
        }
        let mut scoped = binding.clone();
        scoped.providers = vec![provider.clone()];
        Ok(Self {
            adapter,
            binding: scoped,
        })
    }

    pub fn binding(&self) -> &Binding {
        &self.binding
    }

    pub fn provider_name(&self) -> &'static str {
        self.adapter.name()
    }

    /// Never let native CLIs fall back to login/config/metadata credentials
    /// after a missing, invalid, or only partially resolved credential.
    pub fn prepare_env(&self, env: &mut IsolatedEnv) -> Result<()> {
        if !env.secrets_failed.is_empty()
            || self.adapter.cli_required_credentials().iter().any(|key| {
                env.vars.get(*key).is_none_or(|value| {
                    value.trim().is_empty() || value.contains(['\0', '\r', '\n'])
                })
            })
        {
            return Err(LocusError::msg(format!(
                "credential resolve failed for routed {} CLI; a complete credential is required",
                self.adapter.name()
            )));
        }
        self.adapter
            .configure_cli_env(&self.binding.providers[0], &mut env.vars);
        Ok(())
    }
}
