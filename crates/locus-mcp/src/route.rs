//! Launch-time selection of upstreams already authorized by the sealed pin.
//!
//! Routes only narrow authority. They never replace a binding's executable,
//! credential reference, scope, or policy, and discovery never starts workers.

use anyhow::{bail, Result};
use locus_core::{Binding, Session};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Default)]
pub(crate) struct Routes {
    providers: BTreeSet<String>,
}

impl Routes {
    pub(crate) fn insert(&mut self, provider: &str) -> Result<()> {
        // Provider selectors cannot carry tool names, namespace aliases, paths,
        // or commands. Keep diagnostics values-free for malformed input.
        if provider.len() > 64
            || provider.contains("__")
            || !provider
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_lowercase)
            || !provider
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'-')
        {
            bail!("--route requires a lowercase provider name (letters, digits, _ or -; max 64 bytes)");
        }
        self.providers.insert(provider.to_owned());
        Ok(())
    }

    pub(crate) fn is_enabled(&self) -> bool {
        !self.providers.is_empty()
    }

    pub(crate) fn allows_tool(&self, tool: &str) -> bool {
        !self.is_enabled()
            || tool
                .split_once('.')
                .is_some_and(|(provider, _)| self.providers.contains(provider))
    }

    pub(crate) fn validate(&self, session: &Session, bindings: &[(String, Binding)]) -> Result<()> {
        if !self.is_enabled() {
            return Ok(());
        }
        if session.is_namespaced() || bindings.len() != 1 {
            bail!("--route requires one exclusively pinned binding");
        }
        let binding = &bindings[0].1;
        for provider in &self.providers {
            let Some(bound) = binding.provider(provider) else {
                bail!("routed provider `{provider}` is absent from the pinned binding");
            };
            if !bound.has_upstream() {
                bail!("routed provider `{provider}` has no upstream MCP configured in the pinned binding");
            }
        }
        Ok(())
    }
}

/// Same presence-based marker as RQ-LOCUS-SPAWN-LEAK. The existing provider
/// marker also protects workers launched by versions predating that change.
pub(crate) fn is_upstream_child() -> bool {
    std::env::var_os("LOCUS_MCP_UPSTREAM_CHILD").is_some()
        || std::env::var_os("LOCUS_WORKER_PROVIDER").is_some()
}

/// Refuse direct self-reference before even starting a child. The child-side
/// markers remain necessary for shell scripts and other opaque wrappers.
pub(crate) fn check_upstream(binding: &Binding, provider: &str) -> Result<()> {
    let Some(spec) = binding.provider(provider).and_then(|p| p.upstream.as_ref()) else {
        return Ok(());
    };
    let expanded = spec.expand()?;
    let path = std::path::Path::new(&expanded.command);
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let is_self = std::env::current_exe()
        .ok()
        .and_then(|exe| {
            let exe = std::fs::canonicalize(exe).ok()?;
            let command = std::fs::canonicalize(path).ok()?;
            Some(exe == command)
        })
        .unwrap_or(false);
    if is_self
        || matches!(name, "locus-mcp" | "locus-mcp.exe")
        || (matches!(name, "locus" | "locus.exe") && expanded.args.iter().any(|arg| arg == "mcp"))
    {
        bail!(
            "refusing recursive multiplexor launch: an upstream cannot be locus-mcp or locus mcp"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use locus_core::binding::UpstreamSpec;
    use locus_core::{PinSource, SealKey, SessionMode};

    fn fixture() -> (Binding, Session) {
        let binding = Binding::parse_toml(
            "[binding]\nid='bnd_route'\nalias='route'\ntenant='example'\n\
             [[binding.providers]]\nprovider='github'\naccount='example'\n\
             credential_ref='env:ROUTE_FIXTURE'\n\
             [binding.providers.upstream]\ncommand='synthetic-worker-not-launched'\n",
        )
        .unwrap();
        let session = Session::new(
            &binding.id,
            &binding.alias,
            &binding.tenant,
            None,
            PinSource::Explicit,
            None,
            chrono::Duration::minutes(5),
            String::new(),
            &SealKey::from_bytes([7; 32]),
        );
        (binding, session)
    }

    #[test]
    fn routes_require_one_exclusive_binding_and_a_pinned_upstream() {
        let (mut binding, session) = fixture();
        let bindings = vec![(binding.alias.clone(), binding.clone())];
        let mut routes = Routes::default();
        assert!(routes.validate(&session, &[]).is_ok());
        routes.insert("github").unwrap();
        assert!(routes.validate(&session, &bindings).is_ok());
        assert!(routes.validate(&session, &[]).is_err());
        assert!(routes
            .validate(&session, &[bindings[0].clone(), bindings[0].clone()])
            .is_err());
        let namespaced = session
            .clone()
            .with_mode(SessionMode::Namespaced)
            .with_namespaces(vec!["other".into()], vec![]);
        assert!(routes.validate(&namespaced, &bindings).is_err());
        binding.providers[0].upstream = None;
        let unconfigured = [("route".into(), binding)];
        assert!(routes.validate(&session, &unconfigured).is_err());
        routes.insert("supabase").unwrap();
        assert!(routes.validate(&session, &bindings).is_err());
    }

    #[test]
    fn recursive_commands_and_current_executable_aliases_are_refused() {
        let (mut binding, _) = fixture();
        assert!(check_upstream(&binding, "missing").is_ok());
        for (command, args, allowed) in [
            ("/trusted/locus-mcp", vec![], false),
            ("locus-mcp.exe", vec![], false),
            ("locus", vec!["mcp"], false),
            ("locus.exe", vec!["--verbose", "mcp"], false),
            ("locus", vec!["whoami"], true),
            ("synthetic-worker-not-launched", vec!["mcp"], true),
        ] {
            let mut spec = UpstreamSpec::new(command);
            spec.args = args.into_iter().map(str::to_owned).collect();
            binding.providers[0].upstream = Some(spec);
            assert_eq!(check_upstream(&binding, "github").is_ok(), allowed);
        }
        #[cfg(unix)]
        {
            let dir = tempfile::tempdir().unwrap();
            let link = dir.path().join("worker-link");
            std::os::unix::fs::symlink(std::env::current_exe().unwrap(), &link).unwrap();
            binding.providers[0].upstream = Some(UpstreamSpec::new(link.to_str().unwrap()));
            assert!(check_upstream(&binding, "github").is_err());
        }
        binding.providers[0].upstream = None;
        assert!(check_upstream(&binding, "github").is_ok());
    }
}
