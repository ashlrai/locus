use super::{AdapterTool, ProviderAdapter, ToolCallResult};
use crate::binding::{Binding, ProviderBinding};
use crate::error::{LocusError, Result};
use serde_json::{json, Value};

pub struct GithubAdapter;

/// Parse `owner/repo` from args (`full_name`, or `org`/`owner` + `repo`).
fn parse_repo_ref(args: &Value) -> Result<(String, String, String)> {
    if let Some(full) = args.get("full_name").and_then(|v| v.as_str()) {
        let full = full.trim().trim_start_matches("https://github.com/");
        let full = full.trim_start_matches("http://github.com/");
        let full = full.trim_end_matches('/').trim_end_matches(".git");
        let mut parts = full.splitn(2, '/');
        let owner = parts.next().unwrap_or("").trim();
        let repo = parts.next().unwrap_or("").trim();
        if owner.is_empty() || repo.is_empty() {
            return Err(LocusError::msg(
                "github.check_repo: full_name must be `owner/repo`",
            ));
        }
        return Ok((owner.into(), repo.into(), format!("{owner}/{repo}")));
    }

    let owner = args
        .get("org")
        .or_else(|| args.get("owner"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let repo = args
        .get("repo")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());

    match (owner, repo) {
        (Some(o), Some(r)) => {
            // Allow repo as "owner/name" if org omitted-style passed in repo alone.
            if r.contains('/') {
                let mut parts = r.splitn(2, '/');
                let o2 = parts.next().unwrap_or(o);
                let r2 = parts.next().unwrap_or("");
                if r2.is_empty() {
                    return Err(LocusError::msg(
                        "github.check_repo: repo must be a name or owner/name",
                    ));
                }
                Ok((o2.into(), r2.into(), format!("{o2}/{r2}")))
            } else {
                Ok((o.into(), r.into(), format!("{o}/{r}")))
            }
        }
        (None, Some(r)) if r.contains('/') => {
            let mut parts = r.splitn(2, '/');
            let o = parts.next().unwrap_or("");
            let name = parts.next().unwrap_or("");
            if o.is_empty() || name.is_empty() {
                return Err(LocusError::msg(
                    "github.check_repo: repo must be `owner/repo` when org is omitted",
                ));
            }
            Ok((o.into(), name.into(), format!("{o}/{name}")))
        }
        _ => Err(LocusError::msg(
            "github.check_repo: provide full_name, or org+repo (or owner+repo)",
        )),
    }
}

/// Enforce frozen org/repo allowlists. Empty allowlist ⇒ no restriction for that axis.
fn check_repo_allowed(
    owner: &str,
    repo: &str,
    full: &str,
    provider: &ProviderBinding,
) -> Result<()> {
    let orgs = &provider.scope.orgs;
    let repos = &provider.scope.repos;

    if !orgs.is_empty()
        && !orgs
            .iter()
            .any(|o| o.eq_ignore_ascii_case(owner) || o.eq_ignore_ascii_case("*"))
    {
        return Err(LocusError::msg(format!(
            "scope freeze: refusing org/owner={owner:?}; binding allows orgs={orgs:?}"
        )));
    }

    if !repos.is_empty() {
        let allowed = repos.iter().any(|r| {
            let r = r.as_str();
            r.eq_ignore_ascii_case(full)
                || r.eq_ignore_ascii_case(repo)
                || r.eq_ignore_ascii_case(&format!("{owner}/{repo}"))
                || r == "*"
                || (r.ends_with("/*") && r[..r.len() - 2].eq_ignore_ascii_case(owner))
        });
        if !allowed {
            return Err(LocusError::msg(format!(
                "scope freeze: refusing repo={full:?}; binding allows repos={repos:?}"
            )));
        }
    }

    Ok(())
}

impl ProviderAdapter for GithubAdapter {
    fn cli_name(&self) -> Option<&'static str> {
        Some("gh")
    }

    fn name(&self) -> &'static str {
        "github"
    }

    fn tools(&self, provider: &ProviderBinding, binding: &Binding) -> Vec<AdapterTool> {
        let orgs = if provider.scope.orgs.is_empty() {
            "<any>".into()
        } else {
            provider.scope.orgs.join(",")
        };
        let repos = if provider.scope.repos.is_empty() {
            "<any>".into()
        } else {
            provider.scope.repos.join(",")
        };
        vec![
            AdapterTool {
                name: "github.scope".into(),
                description: format!(
                    "Frozen GitHub scope for tenant `{}`: account={}, orgs=[{orgs}], repos=[{repos}].",
                    binding.tenant, provider.account
                ),
                input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
                provider: "github".into(),
                destructive: false,
            },
            AdapterTool {
                name: "github.whoami".into(),
                description: "Show which GitHub identity this pin uses (account + org/repo allowlist). Does not call the GitHub API in phase 1.".into(),
                input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
                provider: "github".into(),
                destructive: false,
            },
            AdapterTool {
                name: "github.check_repo".into(),
                description: format!(
                    "Check whether an org/repo is allowed by this pin's frozen allowlist (orgs=[{orgs}], repos=[{repos}]). Deny if outside scope."
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "full_name": {
                            "type": "string",
                            "description": "owner/repo (e.g. acme-corp/web)"
                        },
                        "org": {
                            "type": "string",
                            "description": "GitHub org or owner (alias of owner)"
                        },
                        "owner": {
                            "type": "string",
                            "description": "GitHub owner (alias of org)"
                        },
                        "repo": {
                            "type": "string",
                            "description": "Repository name, or owner/name"
                        }
                    },
                    "additionalProperties": false
                }),
                provider: "github".into(),
                destructive: false,
            },
        ]
    }

    fn call(
        &self,
        tool: &str,
        args: &Value,
        provider: &ProviderBinding,
        binding: &Binding,
    ) -> Result<ToolCallResult> {
        match tool {
            "github.scope" | "github.whoami" => Ok(ToolCallResult {
                ok: true,
                content: json!({
                    "provider": "github",
                    "account": provider.account,
                    "orgs": provider.scope.orgs,
                    "repos": provider.scope.repos,
                    "credential": crate::credential::credential_metadata(&provider.credential_ref),
                    "tenant": binding.tenant,
                    "binding": binding.alias,
                    "frozen_selectors": ["orgs", "repos"],
                    "worker_hint": "locus exec sets GH_CONFIG_DIR to a private session dir — never mutates global gh auth",
                    "note": "Phase 1 identity tool — real gh/API fan-out uses private config + GH_TOKEN from resolved credential_ref."
                }),
                policy: None,
            }),
            "github.check_repo" => {
                let (owner, repo, full) = parse_repo_ref(args)?;
                check_repo_allowed(&owner, &repo, &full, provider)?;
                Ok(ToolCallResult {
                    ok: true,
                    content: json!({
                        "provider": "github",
                        "allowed": true,
                        "owner": owner,
                        "repo": repo,
                        "full_name": full,
                        "orgs": provider.scope.orgs,
                        "repos": provider.scope.repos,
                        "tenant": binding.tenant,
                        "binding": binding.alias,
                    }),
                    policy: None,
                })
            }
            other => Ok(ToolCallResult {
                ok: false,
                content: json!({"error": format!("unknown github tool: {other}")}),
                policy: None,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::{BindingBody, Policy, Scope};
    use crate::isolation::{
        build_ci_env_map, build_isolated_env, build_isolated_env_for_provider_opts,
        build_isolated_env_opts,
    };
    use crate::seal::SealKey;
    use crate::session::{PinSource, Session};
    use chrono::Duration;
    use std::path::Path;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    const WORKER_HOME: &str = "/tmp/locus-gh-adapter-test";
    const GH_TOKEN_KEYS: [&str; 3] = ["GH_TOKEN", "GITHUB_TOKEN", "GITHUB_PERSONAL_ACCESS_TOKEN"];

    fn github_provider(credential_ref: &str, orgs: &[&str], repos: &[&str]) -> ProviderBinding {
        ProviderBinding {
            provider: "github".into(),
            account: "acme-gh".into(),
            credential_ref: credential_ref.into(),
            scope: Scope {
                orgs: orgs.iter().map(|s| s.to_string()).collect(),
                repos: repos.iter().map(|s| s.to_string()).collect(),
                ..Scope::default()
            },
            upstream: None,
        }
    }

    fn gh_binding(provider: ProviderBinding) -> Binding {
        Binding::from_body(BindingBody {
            id: "bnd_gh_test".into(),
            alias: "gh".into(),
            tenant: "acme-corp".into(),
            principal: None,
            description: None,
            policy: Policy::default(),
            providers: vec![provider],
        })
    }

    fn test_session(binding: &Binding) -> Session {
        let key = SealKey::generate();
        Session::new(
            &binding.id,
            &binding.alias,
            &binding.tenant,
            None,
            PinSource::Explicit,
            None,
            Duration::hours(1),
            WORKER_HOME.into(),
            &key,
        )
    }

    /// Serializes env-mutating tests in this module. Other suites never touch
    /// the `GH_*` names, so a module-local lock is sufficient.
    fn env_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// Save/restore guard for process env mutated by a test — real `GH_*`
    /// variables may exist on a developer machine and must survive the run.
    struct EnvGuard(Vec<(&'static str, Option<String>)>);

    impl EnvGuard {
        fn capture(keys: &[&'static str]) -> Self {
            Self(keys.iter().map(|k| (*k, std::env::var(k).ok())).collect())
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (k, v) in &self.0 {
                match v {
                    Some(v) => std::env::set_var(k, v),
                    None => std::env::remove_var(k),
                }
            }
        }
    }

    // ── parse_repo_ref ────────────────────────────────────────────────

    #[test]
    fn parse_repo_ref_full_name_spellings() {
        for (input, want) in [
            ("acme-corp/web", "acme-corp/web"),
            ("https://github.com/acme-corp/web", "acme-corp/web"),
            ("http://github.com/acme-corp/web", "acme-corp/web"),
            ("https://github.com/acme-corp/web.git", "acme-corp/web"),
            ("acme-corp/web/", "acme-corp/web"),
            ("  acme-corp/web  ", "acme-corp/web"),
        ] {
            let (owner, repo, full) = parse_repo_ref(&json!({ "full_name": input })).unwrap();
            assert_eq!(full, want, "input {input:?}");
            assert_eq!(format!("{owner}/{repo}"), want);
        }
    }

    #[test]
    fn parse_repo_ref_full_name_requires_owner_and_repo() {
        for input in [
            "",
            "acme-only",
            "acme-corp/",
            "/web",
            "https://github.com/acme-corp",
        ] {
            let err = parse_repo_ref(&json!({ "full_name": input }))
                .unwrap_err()
                .to_string();
            assert!(err.contains("full_name"), "input {input:?} → {err}");
        }
    }

    #[test]
    fn parse_repo_ref_owner_repo_arg_spellings() {
        let (o, r, full) = parse_repo_ref(&json!({ "org": "acme-corp", "repo": "web" })).unwrap();
        assert_eq!(
            (o.as_str(), r.as_str(), full.as_str()),
            ("acme-corp", "web", "acme-corp/web")
        );
        // `owner` is an alias of `org`.
        let (_, _, full) = parse_repo_ref(&json!({ "owner": "acme-corp", "repo": "api" })).unwrap();
        assert_eq!(full, "acme-corp/api");
        // An owner embedded in `repo` wins over the org selector.
        let (o, r, full) =
            parse_repo_ref(&json!({ "org": "ignored-org", "repo": "acme-corp/web" })).unwrap();
        assert_eq!(
            (o.as_str(), r.as_str(), full.as_str()),
            ("acme-corp", "web", "acme-corp/web")
        );
        // `repo` alone may carry `owner/name`.
        let (_, _, full) = parse_repo_ref(&json!({ "repo": "acme-corp/api" })).unwrap();
        assert_eq!(full, "acme-corp/api");
    }

    #[test]
    fn parse_repo_ref_rejects_incomplete_args() {
        for args in [
            json!({}),
            json!({ "org": "acme-corp" }),   // owner without repo
            json!({ "repo": "web" }),        // bare repo, no owner
            json!({ "repo": "acme-corp/" }), // empty name after slash
            json!({ "org": "   ", "repo": "web" }), // blank org counts as missing
        ] {
            let err = parse_repo_ref(&args).unwrap_err().to_string();
            assert!(err.contains("github.check_repo"), "{args} → {err}");
        }
    }

    // ── check_repo_allowed ────────────────────────────────────────────

    #[test]
    fn check_repo_allowed_empty_scope_allows_anything() {
        let p = github_provider("phm:GH_ACME", &[], &[]);
        check_repo_allowed("evil-corp", "anything", "evil-corp/anything", &p).unwrap();
    }

    #[test]
    fn check_repo_allowed_enforces_org_and_repo_axes() {
        let p = github_provider("phm:GH_ACME", &["acme-corp"], &["acme-corp/web"]);
        check_repo_allowed("acme-corp", "web", "acme-corp/web", &p).unwrap();

        // A listed org does not open unlisted repos.
        let err = check_repo_allowed("acme-corp", "api", "acme-corp/api", &p)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("scope freeze") && err.contains("repo"),
            "{err}"
        );

        // A listed repo does not open unlisted orgs (org axis denies first).
        let err = check_repo_allowed("evil-corp", "web", "evil-corp/web", &p)
            .unwrap_err()
            .to_string();
        assert!(err.contains("scope freeze") && err.contains("org"), "{err}");
    }

    #[test]
    fn check_repo_allowed_wildcards_and_case_insensitivity() {
        // `*` on both axes opens everything.
        let p = github_provider("phm:GH_ACME", &["*"], &["*"]);
        check_repo_allowed("any-org", "any-repo", "any-org/any-repo", &p).unwrap();

        // `owner/*` repo pattern + case-insensitive compare on both axes.
        let p = github_provider("phm:GH_ACME", &["Acme-Corp"], &["acme-corp/*"]);
        check_repo_allowed("ACME-CORP", "Web", "ACME-CORP/Web", &p).unwrap();
        let err = check_repo_allowed("other-org", "web", "other-org/web", &p)
            .unwrap_err()
            .to_string();
        assert!(err.contains("scope freeze"), "{err}");

        // A bare repo name in the allowlist matches the repo axis alone.
        let p = github_provider("phm:GH_ACME", &[], &["web"]);
        check_repo_allowed("any-org", "web", "any-org/web", &p).unwrap();
        let err = check_repo_allowed("any-org", "api", "any-org/api", &p)
            .unwrap_err()
            .to_string();
        assert!(err.contains("repo"), "{err}");
    }

    // ── tools() catalog ───────────────────────────────────────────────

    #[test]
    fn tools_catalog_is_read_only_and_scope_aware() {
        let p = github_provider("phm:GH_ACME", &["acme-corp"], &["acme-corp/web"]);
        let b = gh_binding(p.clone());
        let tools = GithubAdapter.tools(&p, &b);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["github.scope", "github.whoami", "github.check_repo"]
        );
        for t in &tools {
            assert!(!t.destructive, "{} must stay read-only", t.name);
            assert_eq!(t.provider, "github");
            assert_eq!(t.input_schema["additionalProperties"], json!(false));
        }
        // scope/check_repo embed the frozen allowlist in their descriptions;
        // whoami is a static blurb.
        for t in tools.iter().filter(|t| t.name != "github.whoami") {
            assert!(t.description.contains("acme-corp"), "{}", t.description);
        }
        let check = tools
            .iter()
            .find(|t| t.name == "github.check_repo")
            .unwrap();
        let props = check.input_schema["properties"].as_object().unwrap();
        for key in ["full_name", "org", "owner", "repo"] {
            assert!(props.contains_key(key), "missing schema arg {key}");
        }

        // An unfrozen scope is described as <any>, never left blank.
        let open = github_provider("phm:GH_ACME", &[], &[]);
        let b2 = gh_binding(open.clone());
        let scope_tool = GithubAdapter
            .tools(&open, &b2)
            .into_iter()
            .find(|t| t.name == "github.scope")
            .unwrap();
        assert!(
            scope_tool.description.contains("<any>"),
            "{}",
            scope_tool.description
        );
    }

    // ── call() ────────────────────────────────────────────────────────

    #[test]
    fn whoami_reports_pin_identity_without_credential_material() {
        let p = github_provider("phm:GH_ACME_CANARY", &["acme-corp"], &["acme-corp/web"]);
        let b = gh_binding(p.clone());
        for tool in ["github.scope", "github.whoami"] {
            let r = GithubAdapter.call(tool, &json!({}), &p, &b).unwrap();
            assert!(r.ok, "{tool} must succeed");
            assert_eq!(r.content["account"], json!("acme-gh"));
            assert_eq!(r.content["orgs"], json!(["acme-corp"]));
            assert_eq!(r.content["repos"], json!(["acme-corp/web"]));
            assert_eq!(r.content["tenant"], json!("acme-corp"));
            assert_eq!(r.content["binding"], json!("gh"));
            assert_eq!(r.content["frozen_selectors"], json!(["orgs", "repos"]));
            assert_eq!(r.content["credential"]["source"], json!("phantom"));
            assert_eq!(r.content["credential"]["present"], json!(true));
            // The credential locator name never crosses into tool output.
            let raw = serde_json::to_string(&r.content).unwrap();
            assert!(
                !raw.contains("GH_ACME_CANARY"),
                "{tool} leaked credential_ref"
            );
            assert!(r.content.get("credential_ref").is_none());
        }

        // `env:` refs report their source without naming the variable.
        let p = github_provider("env:LOCUS_GH_SOURCE_XYZ", &[], &[]);
        let b = gh_binding(p.clone());
        let r = GithubAdapter
            .call("github.whoami", &json!({}), &p, &b)
            .unwrap();
        assert_eq!(r.content["credential"]["source"], json!("environment"));
        let raw = serde_json::to_string(&r.content).unwrap();
        assert!(!raw.contains("LOCUS_GH_SOURCE_XYZ"));
    }

    #[test]
    fn call_check_repo_round_trips_full_name_and_denies_hard() {
        let p = github_provider("phm:GH_ACME", &["acme-corp"], &["acme-corp/web"]);
        let b = gh_binding(p.clone());
        let r = GithubAdapter
            .call(
                "github.check_repo",
                &json!({ "full_name": "acme-corp/web" }),
                &p,
                &b,
            )
            .unwrap();
        assert!(r.ok);
        assert_eq!(r.content["allowed"], json!(true));
        assert_eq!(r.content["full_name"], json!("acme-corp/web"));

        // Out-of-scope is a hard error, not a soft `ok: false`.
        let err = GithubAdapter
            .call(
                "github.check_repo",
                &json!({ "full_name": "evil-corp/web" }),
                &p,
                &b,
            )
            .unwrap_err()
            .to_string();
        assert!(err.contains("scope freeze"), "{err}");
    }

    #[test]
    fn call_unknown_tool_denies_without_throwing() {
        let p = github_provider("phm:GH_ACME", &[], &[]);
        let b = gh_binding(p.clone());
        let r = GithubAdapter
            .call("github.delete_everything", &json!({}), &p, &b)
            .unwrap();
        assert!(!r.ok);
        let err = r.content["error"].as_str().unwrap();
        assert!(err.contains("unknown github tool"), "{err}");
    }

    // ── env injection + ambient scrubbing (worker surface) ────────────

    #[test]
    fn github_inject_keys_are_the_standard_token_env_names() {
        let keys = crate::credential::inject_keys_for_provider("github");
        for k in GH_TOKEN_KEYS {
            assert!(keys.contains(&k), "github must inject {k}");
        }
        // Provider matching is case-insensitive (TOML spellings vary).
        assert_eq!(crate::credential::inject_keys_for_provider("GitHub"), keys);
        // A builtin (non-community) provider binding maps to the same set.
        let p = github_provider("env:LOCUS_GH_KEYS_XYZ", &[], &[]);
        let got = crate::credential::inject_keys_for_binding_provider(&p).unwrap();
        assert_eq!(got, keys.iter().map(|k| k.to_string()).collect::<Vec<_>>());
    }

    #[test]
    fn isolated_env_sets_private_gh_config_and_frozen_scope() {
        let p = github_provider(
            "phm:GH_ACME",
            &["acme-corp", "acme-lab"],
            &["acme-corp/web"],
        );
        let b = gh_binding(p);
        let s = test_session(&b);
        let iso = build_isolated_env(&s, &b);

        // Private gh config root — the user's global `gh` auth is never touched.
        let want = Path::new(WORKER_HOME).join("gh").display().to_string();
        assert_eq!(
            iso.vars.get("GH_CONFIG_DIR").map(String::as_str),
            Some(want.as_str())
        );

        // Frozen scope + account surface under LOCUS_GITHUB_*.
        assert_eq!(
            iso.vars.get("LOCUS_GITHUB_ACCOUNT").map(String::as_str),
            Some("acme-gh")
        );
        assert_eq!(
            iso.vars.get("LOCUS_GITHUB_ORGS").map(String::as_str),
            Some("acme-corp,acme-lab")
        );
        assert_eq!(
            iso.vars.get("LOCUS_GITHUB_REPOS").map(String::as_str),
            Some("acme-corp/web")
        );
        assert_eq!(
            iso.vars.get("LOCUS_PROVIDERS").map(String::as_str),
            Some("github")
        );
        assert_eq!(
            iso.vars
                .get("LOCUS_GITHUB_CREDENTIAL_RESOLVED")
                .map(String::as_str),
            Some("0")
        );

        // The credential locator name is never exported to the child.
        assert!(!iso.vars.keys().any(|k| k.contains("CREDENTIAL_REF")));
        assert!(!iso.vars.values().any(|v| v.contains("GH_ACME")));
    }

    #[test]
    fn isolated_env_scrubs_ambient_github_identity() {
        let _lock = env_lock();
        let _guard = EnvGuard::capture(&[
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "GITHUB_PERSONAL_ACCESS_TOKEN",
            "GH_CONFIG_DIR",
        ]);
        std::env::set_var("GH_TOKEN", "gho_ambient_canary");
        std::env::set_var("GITHUB_TOKEN", "ghp_ambient_canary");
        std::env::set_var("GITHUB_PERSONAL_ACCESS_TOKEN", "pat_ambient_canary");
        std::env::set_var("GH_CONFIG_DIR", "/tmp/ambient-gh-config");

        let p = github_provider("phm:GH_ACME", &["acme-corp"], &[]);
        let b = gh_binding(p);
        let s = test_session(&b);
        // Secrets not resolved — nothing ambient may stand in for the pin.
        let iso = build_isolated_env(&s, &b);

        for k in GH_TOKEN_KEYS {
            assert!(
                !iso.vars.contains_key(k),
                "ambient {k} must not cross into the child"
            );
            assert!(
                iso.scrubbed.iter().any(|s| s == k),
                "{k} must be reported scrubbed"
            );
        }
        // The private config dir always wins over the ambient one.
        let want = Path::new(WORKER_HOME).join("gh").display().to_string();
        assert_eq!(
            iso.vars.get("GH_CONFIG_DIR").map(String::as_str),
            Some(want.as_str())
        );
        assert!(iso.scrubbed.iter().any(|s| s == "GH_CONFIG_DIR"));
        // Ambient tokens never mark the pinned credential resolved.
        assert_eq!(
            iso.vars
                .get("LOCUS_GITHUB_CREDENTIAL_RESOLVED")
                .map(String::as_str),
            Some("0")
        );
        for canary in [
            "gho_ambient_canary",
            "ghp_ambient_canary",
            "pat_ambient_canary",
            "/tmp/ambient-gh-config",
        ] {
            assert!(!iso.vars.values().any(|v| v == canary), "{canary} leaked");
        }
    }

    #[test]
    fn isolated_env_injects_resolved_secret_under_gh_env_names() {
        let _lock = env_lock();
        let _guard = EnvGuard::capture(&[
            "LOCUS_GH_ADAPTER_CRED",
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "GITHUB_PERSONAL_ACCESS_TOKEN",
        ]);
        std::env::set_var("LOCUS_GH_ADAPTER_CRED", "gho_locus_adapter_test_secret");

        let p = github_provider("env:LOCUS_GH_ADAPTER_CRED", &["acme-corp"], &[]);
        let b = gh_binding(p);
        let s = test_session(&b);
        let iso = build_isolated_env_opts(&s, &b, true);

        // One resolved value fans out to every standard gh token name.
        for k in GH_TOKEN_KEYS {
            assert_eq!(
                iso.vars.get(k).map(String::as_str),
                Some("gho_locus_adapter_test_secret"),
                "{k} must carry the resolved secret"
            );
        }
        assert_eq!(iso.secrets_resolved, 3);
        assert_eq!(
            iso.vars
                .get("LOCUS_GITHUB_CREDENTIAL_RESOLVED")
                .map(String::as_str),
            Some("1")
        );
        // The locator variable itself is scrubbed — the child sees only the
        // standard names.
        assert!(!iso.vars.contains_key("LOCUS_GH_ADAPTER_CRED"));
        assert!(iso.secrets_failed.is_empty(), "{:?}", iso.secrets_failed);
    }

    #[test]
    fn isolated_env_unset_env_ref_reports_unavailable() {
        let _lock = env_lock();
        let _guard = EnvGuard::capture(&["LOCUS_GH_ADAPTER_MISSING"]);
        std::env::remove_var("LOCUS_GH_ADAPTER_MISSING");

        let p = github_provider("env:LOCUS_GH_ADAPTER_MISSING", &[], &[]);
        let b = gh_binding(p);
        let s = test_session(&b);
        let iso = build_isolated_env_opts(&s, &b, true);

        assert!(!iso.vars.contains_key("GH_TOKEN"));
        assert_eq!(
            iso.vars
                .get("LOCUS_GITHUB_CREDENTIAL_RESOLVED")
                .map(String::as_str),
            Some("0")
        );
        assert!(
            iso.secrets_failed.iter().any(|i| i.provider == "github"
                && i.source == "environment"
                && i.code == "unavailable"),
            "{:?}",
            iso.secrets_failed
        );
    }

    #[test]
    fn isolated_env_phm_ref_fails_closed_without_injection() {
        let p = github_provider("phm:GH_ACME", &["acme-corp"], &[]);
        let b = gh_binding(p);
        let s = test_session(&b);
        let iso = build_isolated_env_opts(&s, &b, true);

        // No scoped phantom bridge exists — the ref stays unresolved and the
        // failure is reported as a safe issue, never as injected env.
        for k in GH_TOKEN_KEYS {
            assert!(
                !iso.vars.contains_key(k),
                "{k} must not be injected for phm:"
            );
        }
        assert_eq!(
            iso.vars
                .get("LOCUS_GITHUB_CREDENTIAL_RESOLVED")
                .map(String::as_str),
            Some("0")
        );
        assert!(
            iso.secrets_failed
                .iter()
                .any(|i| i.provider == "github" && i.code == "unsupported-integration"),
            "{:?}",
            iso.secrets_failed
        );
    }

    #[test]
    fn provider_scoped_env_carries_only_github_credentials() {
        let _lock = env_lock();
        let _guard = EnvGuard::capture(&["LOCUS_GH_ADAPTER_CRED", "LOCUS_GH_ADAPTER_SB_CRED"]);
        std::env::set_var("LOCUS_GH_ADAPTER_CRED", "gho_locus_adapter_test_secret");
        std::env::set_var("LOCUS_GH_ADAPTER_SB_CRED", "sbp_locus_adapter_test_secret");

        let mut b = gh_binding(github_provider(
            "env:LOCUS_GH_ADAPTER_CRED",
            &["acme-corp"],
            &[],
        ));
        b.providers.push(ProviderBinding {
            provider: "supabase".into(),
            account: "acme-sb".into(),
            credential_ref: "env:LOCUS_GH_ADAPTER_SB_CRED".into(),
            scope: Scope {
                project_ref: Some("proj_acme".into()),
                ..Scope::default()
            },
            upstream: None,
        });
        let s = test_session(&b);
        let gh = b.provider("github").unwrap().clone();
        let iso = build_isolated_env_for_provider_opts(&s, &b, &gh, true);

        assert_eq!(
            iso.vars.get("LOCUS_PROVIDERS").map(String::as_str),
            Some("github")
        );
        assert_eq!(
            iso.vars.get("GH_TOKEN").map(String::as_str),
            Some("gho_locus_adapter_test_secret")
        );
        // No cross-provider secret or scope material reaches the worker.
        assert!(!iso.vars.contains_key("SUPABASE_ACCESS_TOKEN"));
        assert!(!iso.vars.contains_key("SUPABASE_PROJECT_REF"));
        assert!(!iso.vars.keys().any(|k| k.starts_with("LOCUS_SUPABASE_")));
        assert!(!iso
            .vars
            .values()
            .any(|v| v == "sbp_locus_adapter_test_secret" || v == "proj_acme"));
    }

    #[test]
    fn ci_env_map_exports_scope_and_private_config_without_secrets() {
        let _lock = env_lock();
        let _guard = EnvGuard::capture(&["LOCUS_GH_ADAPTER_CRED", "LOCUS_CI_ALLOW_SECRETS"]);
        std::env::set_var("LOCUS_GH_ADAPTER_CRED", "gho_locus_adapter_test_secret");
        std::env::remove_var("LOCUS_CI_ALLOW_SECRETS");

        let p = github_provider(
            "env:LOCUS_GH_ADAPTER_CRED",
            &["acme-corp"],
            &["acme-corp/web"],
        );
        let b = gh_binding(p);
        let s = test_session(&b);
        let map = build_ci_env_map(&s, &b, true);

        let want = Path::new(WORKER_HOME).join("gh").display().to_string();
        assert_eq!(
            map.get("GH_CONFIG_DIR").map(String::as_str),
            Some(want.as_str())
        );
        assert_eq!(
            map.get("LOCUS_GITHUB_ACCOUNT").map(String::as_str),
            Some("acme-gh")
        );
        assert_eq!(
            map.get("LOCUS_GITHUB_ORGS").map(String::as_str),
            Some("acme-corp")
        );
        // Without the explicit CI opt-in, resolved secrets never reach the export.
        assert!(!map.contains_key("GH_TOKEN"));
        assert!(!map.values().any(|v| v == "gho_locus_adapter_test_secret"));
        assert_eq!(
            map.get("LOCUS_GITHUB_CREDENTIAL_RESOLVED")
                .map(String::as_str),
            Some("0")
        );
    }
}
