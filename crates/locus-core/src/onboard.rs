//! Guided multi-account binding setup: ambient-identity detection + wizard plan state.
//!
//! The interactive wizard lives in `locus-cli` (`locus onboard`); this module
//! holds the testable, dependency-free core:
//!
//! - [`detect_ambient_candidates`] — probe the machine for ambient identity
//!   (GitHub CLI accounts, AWS profiles, well-known env vars, `~/.ashlr/config.json`,
//!   `.mcp.json`) and return *candidates*. The wizard never auto-pins anything:
//!   every candidate is presented to the operator for an explicit accept/reject.
//! - [`OnboardPlan`] — resumable wizard state (`$LOCUS_HOME/onboard-plan.json`).
//!   Refs only; secret values never touch this file.
//! - [`PlannedBinding`] — one wizard-produced binding draft, convertible to a
//!   real [`Binding`](crate::binding::Binding) via [`PlannedBinding::into_binding`].
//!
//! Detection is best-effort and read-only. A probe that fails (missing binary,
//! unreadable file, malformed JSON) is skipped silently — absence of a
//! candidate is never an error.

use crate::binding::{Binding, BindingBody, Policy, ProviderBinding, Scope};
use crate::credential::CredentialRef;
use crate::error::{LocusError, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Where the wizard keeps its resumable plan (refs only, never secret values).
pub const ONBOARD_PLAN_FILE: &str = "onboard-plan.json";

/// One ambient-identity candidate the wizard may convert into a binding.
///
/// `account_hint` is a display label only (a username, profile name, env var
/// name). It is never a secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectedCandidate {
    /// Probe that found it: `gh`, `aws-config`, `env`, `ashlr-config`, `mcp-json`.
    pub source: String,
    /// Locus provider id (`github`, `aws`, `supabase`, …).
    pub provider: String,
    /// Display label for the account (username, profile, var name).
    pub account_hint: String,
    /// One-line human detail (never contains secret material).
    pub detail: String,
}

impl DetectedCandidate {
    fn new(
        source: impl Into<String>,
        provider: impl Into<String>,
        account_hint: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            source: source.into(),
            provider: provider.into(),
            account_hint: account_hint.into(),
            detail: detail.into(),
        }
    }
}

/// Probe the machine for ambient identity and return de-duplicated candidates.
///
/// Never fails: every probe is best-effort. `home` is the OS home directory
/// (for `~/.aws/config`, `~/.ashlr/config.json`); `cwd` is scanned for
/// `.mcp.json`.
pub fn detect_ambient_candidates(home: &Path, cwd: &Path) -> Vec<DetectedCandidate> {
    let mut out = Vec::new();
    detect_gh_cli(&mut out);
    detect_aws_config(home, &mut out);
    detect_env_vars(&mut out);
    detect_ashlr_config(home, &mut out);
    detect_mcp_json(cwd, &mut out);
    dedupe_candidates(out)
}

fn dedupe_candidates(cands: Vec<DetectedCandidate>) -> Vec<DetectedCandidate> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for c in cands {
        let key = format!("{}|{}|{}", c.source, c.provider, c.account_hint);
        if seen.insert(key) {
            out.push(c);
        }
    }
    out
}

const PROBE_DEADLINE: std::time::Duration = std::time::Duration::from_millis(500);
const PROBE_OUTPUT_LIMIT: u64 = 16 * 1024;
static PROBE_READERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

struct ProbeReaderPermit;
impl Drop for ProbeReaderPermit {
    fn drop(&mut self) {
        PROBE_READERS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

fn stop_probe_child(child: &mut std::process::Child) {
    // The group was created exclusively for this probe. Its descendants may
    // still own output pipes after the leader exits; terminate the group first.
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    if child.try_wait().ok().flatten().is_some() {
        return;
    }
    let _ = child.kill();
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(50);
    while std::time::Instant::now() < deadline {
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// One bounded metadata probe, including both stdout and stderr. Reader slots
/// remain reserved until an escaped grandchild closes its pipe; repeated probes
/// cannot accumulate unbounded blocked threads. Captured text is never emitted.
fn run_probe(program: &str, args: &[&str]) -> Option<zeroize::Zeroizing<String>> {
    use std::io::Read;
    use std::sync::atomic::Ordering;
    use std::sync::mpsc;
    use zeroize::Zeroize;
    PROBE_READERS
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |count| {
            if count <= 14 {
                Some(count + 2)
            } else {
                None
            }
        })
        .ok()?;
    let permits = [ProbeReaderPermit, ProbeReaderPermit];
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn().ok()?;
    let stdout = child.stdout.take()?;
    let stderr = child.stderr.take()?;
    let (sender, receiver) = mpsc::sync_channel(2);
    fn read_pipe(pipe: impl Read, permit: ProbeReaderPermit) -> Option<zeroize::Zeroizing<String>> {
        let _permit = permit;
        let mut bytes = Vec::new();
        let result = pipe.take(PROBE_OUTPUT_LIMIT + 1).read_to_end(&mut bytes);
        if result.is_err() || bytes.len() as u64 > PROBE_OUTPUT_LIMIT {
            bytes.zeroize();
            return None;
        }
        match String::from_utf8(bytes) {
            Ok(text) => Some(zeroize::Zeroizing::new(text)),
            Err(error) => {
                error.into_bytes().zeroize();
                None
            }
        }
    }
    let [out_permit, err_permit] = permits;
    let out_sender = sender.clone();
    if std::thread::Builder::new()
        .name("locus-probe-stdout".into())
        .spawn(move || {
            let _ = out_sender.send((0, read_pipe(stdout, out_permit)));
        })
        .is_err()
    {
        stop_probe_child(&mut child);
        return None;
    }
    if std::thread::Builder::new()
        .name("locus-probe-stderr".into())
        .spawn(move || {
            let _ = sender.send((1, read_pipe(stderr, err_permit)));
        })
        .is_err()
    {
        stop_probe_child(&mut child);
        return None;
    }
    let deadline = std::time::Instant::now() + PROBE_DEADLINE;
    let mut output = [None, None];
    let mut received = 0;
    let mut status = None;
    while std::time::Instant::now() < deadline {
        while let Ok((index, text)) = receiver.try_recv() {
            output[index] = text;
            received += 1;
        }
        status = status.or_else(|| child.try_wait().ok().flatten());
        if received == 2 && status.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    if status.is_none() || received != 2 {
        stop_probe_child(&mut child);
    }
    if !status.is_some_and(|status| status.success()) || received != 2 {
        return None;
    }
    Some(zeroize::Zeroizing::new(format!(
        "{}\n{}",
        output[0].as_deref()?,
        output[1].as_deref()?
    )))
}

/// `gh auth status` → one candidate per logged-in account; one bounded command.
fn detect_gh_cli(out: &mut Vec<DetectedCandidate>) {
    detect_gh_cli_using(out, "gh");
}

fn detect_gh_cli_using(out: &mut Vec<DetectedCandidate>, program: &str) {
    let Some(text) = run_probe(program, &["auth", "status"]) else {
        return;
    };
    for line in text.lines() {
        let Some(rest) = line.split("Logged in to github.com account ").nth(1) else {
            continue;
        };
        let user = rest.split_whitespace().next().unwrap_or("");
        if !user.is_empty()
            && user.len() <= 39
            && user
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            out.push(DetectedCandidate::new(
                "gh",
                "github",
                user,
                format!("gh CLI logged in as {user}"),
            ));
        }
    }
}

/// `~/.aws/config` `[profile name]` sections → `aws` candidates.
fn detect_aws_config(home: &Path, out: &mut Vec<DetectedCandidate>) {
    let path = home.join(".aws").join("config");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    for line in text.lines() {
        let line = line.trim();
        let profile = line
            .strip_prefix("[profile ")
            .and_then(|s| s.strip_suffix(']'))
            .or_else(|| (line == "[default]").then_some("default"));
        if let Some(name) = profile {
            let name = name.trim();
            if !name.is_empty() {
                out.push(DetectedCandidate::new(
                    "aws-config",
                    "aws",
                    name,
                    format!("AWS config profile [{name}]"),
                ));
            }
        }
    }
}

/// Well-known env vars → provider candidates. The *var name* is the hint;
/// values are never read.
fn detect_env_vars(out: &mut Vec<DetectedCandidate>) {
    const PROBES: &[(&str, &str)] = &[
        ("GH_TOKEN", "github"),
        ("GITHUB_TOKEN", "github"),
        ("AWS_PROFILE", "aws"),
        ("AWS_ACCESS_KEY_ID", "aws"),
        ("SUPABASE_URL", "supabase"),
        ("SUPABASE_ANON_KEY", "supabase"),
        ("SUPABASE_SERVICE_ROLE_KEY", "supabase"),
        ("VERCEL_TOKEN", "vercel"),
        ("CLOUDFLARE_API_TOKEN", "cloudflare"),
        ("STRIPE_API_KEY", "stripe"),
        ("STRIPE_SECRET_KEY", "stripe"),
        ("RESEND_API_KEY", "resend"),
        ("OPENAI_API_KEY", "openai"),
        ("ANTHROPIC_API_KEY", "anthropic"),
    ];
    for &(var, provider) in PROBES {
        if std::env::var_os(var).is_some() {
            out.push(DetectedCandidate::new(
                "env",
                provider,
                var,
                format!("ambient ${var} is set"),
            ));
        }
    }
}

/// `~/.ashlr/config.json` → best-effort account labels (keys named like
/// `accounts`, `profiles`, or string-valued entries).
fn detect_ashlr_config(home: &Path, out: &mut Vec<DetectedCandidate>) {
    let path = home.join(".ashlr").join("config.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
    // Collect string labels from a few conventional shapes; never values
    // that look like secrets (only object keys / short labels are used).
    let mut labels: Vec<String> = Vec::new();
    if let Some(obj) = v.as_object() {
        for key in ["accounts", "profiles", "tenants", "workspaces"] {
            if let Some(arr) = obj.get(key).and_then(|x| x.as_array()) {
                for item in arr {
                    if let Some(s) = item.as_str() {
                        labels.push(s.to_string());
                    } else if let Some(name) = item.get("name").and_then(|x| x.as_str()) {
                        labels.push(name.to_string());
                    }
                }
            }
        }
    }
    for label in labels {
        let label = label.trim();
        if label.is_empty() || label.len() > 64 {
            continue;
        }
        out.push(DetectedCandidate::new(
            "ashlr-config",
            "custom",
            label,
            format!("~/.ashlr/config.json lists {label}"),
        ));
    }
}

/// `<cwd>/.mcp.json` `mcpServers` keys → provider-name candidates.
fn detect_mcp_json(cwd: &Path, out: &mut Vec<DetectedCandidate>) {
    let path = cwd.join(".mcp.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
    let servers = v
        .get("mcpServers")
        .and_then(|x| x.as_object())
        .or_else(|| v.as_object());
    let Some(servers) = servers else { return };
    for name in servers.keys() {
        let name = name.trim();
        if name.is_empty() || name.len() > 64 {
            continue;
        }
        out.push(DetectedCandidate::new(
            "mcp-json",
            name.to_ascii_lowercase(),
            name,
            format!(".mcp.json declares MCP server {name}"),
        ));
    }
}

/// Suggest a supported environment credential-ref name for a provider × account pair.
/// Uppercase, non-alphanumeric → `_`; never includes secret material.
pub fn suggest_credential_ref(provider: &str, account: &str) -> String {
    let clean = |s: &str| {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_uppercase()
                } else {
                    '_'
                }
            })
            .collect::<String>()
    };
    format!("env:{}_{}", clean(provider), clean(account))
}

/// Suggest a binding alias from a candidate (lowercase, slugified).
pub fn suggest_alias(provider: &str, account_hint: &str) -> String {
    let slug = account_hint
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if slug.is_empty() || slug == provider {
        provider.to_string()
    } else {
        format!("{provider}-{slug}")
    }
}

/// Scope fields worth freezing for a provider (mirrors the CLI's guided prompts).
pub fn scope_fields_for_provider(provider: &str) -> &'static [&'static str] {
    match provider.to_ascii_lowercase().as_str() {
        "supabase" => &["project_ref"],
        "vercel" => &["team_id", "project_ref"],
        "github" => &["org", "repos"],
        "aws" | "stripe" | "cloudflare" => &["account_id"],
        _ => &[],
    }
}

/// One wizard-produced binding draft.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedBinding {
    pub alias: String,
    pub tenant: String,
    pub provider: String,
    pub account: String,
    pub credential_ref: String,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org: Option<String>,
    #[serde(default)]
    pub repos: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl PlannedBinding {
    /// Validate refs-only invariants before anything is written.
    pub fn validate(&self) -> Result<()> {
        if self.alias.trim().is_empty() {
            return Err(LocusError::msg("binding alias is empty"));
        }
        if self.provider.trim().is_empty() {
            return Err(LocusError::msg("provider is empty"));
        }
        if self.account.trim().is_empty() {
            return Err(LocusError::msg("account is empty"));
        }
        // Rejects bare names, raw tokens, empty refs — same gate as bindings.
        CredentialRef::validate(&self.credential_ref).map_err(|e| {
            LocusError::msg(format!(
                "credential_ref for `{}` rejected: {e} (use phm:NAME or env:VAR)",
                self.alias
            ))
        })?;
        Ok(())
    }

    /// Convert into a real [`Binding`] (single provider).
    pub fn into_binding(self) -> Result<Binding> {
        self.validate()?;
        let mut scope = Scope {
            project_ref: self.project_ref.clone(),
            team_id: self.team_id.clone(),
            account_id: self.account_id.clone(),
            read_only: if self.read_only { Some(true) } else { None },
            ..Scope::default()
        };
        if let Some(o) = &self.org {
            scope.orgs = vec![o.clone()];
        }
        scope.repos = self.repos.clone();
        Ok(Binding::from_body(BindingBody {
            id: format!("bnd_{}", self.alias),
            alias: self.alias,
            tenant: self.tenant,
            principal: None,
            description: self
                .description
                .or_else(|| Some("created by locus onboard".to_string())),
            policy: Policy::default(),
            providers: vec![ProviderBinding {
                provider: self.provider,
                account: self.account,
                credential_ref: self.credential_ref,
                scope,
                upstream: None,
            }],
        }))
    }
}

/// Wizard steps, in order. `completed` holds the steps already done.
pub mod steps {
    pub const DETECT: &str = "detect";
    pub const TENANTS: &str = "tenants";
    pub const CREDENTIALS: &str = "credentials";
    pub const SCOPES: &str = "scopes";
    pub const WORKSPACES: &str = "workspaces";
    pub const VERIFY: &str = "verify";

    pub const ALL: &[&str] = &[DETECT, TENANTS, CREDENTIALS, SCOPES, WORKSPACES, VERIFY];
}

/// Resumable wizard state. Stored at `$LOCUS_HOME/onboard-plan.json` (mode 0600).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OnboardPlan {
    pub version: u32,
    #[serde(default)]
    pub completed: Vec<String>,
    #[serde(default)]
    pub bindings: Vec<PlannedBinding>,
    /// Candidate selections from the detect step (`source|provider|hint` keys).
    #[serde(default)]
    pub selected: Vec<String>,
    /// Workspace roots the operator asked to provision (`.locus.toml`).
    #[serde(default)]
    pub workspaces: Vec<String>,
}

impl OnboardPlan {
    pub fn plan_path(home: &Path) -> PathBuf {
        home.join(ONBOARD_PLAN_FILE)
    }

    pub fn load(home: &Path) -> Result<Self> {
        let path = Self::plan_path(home);
        if !path.exists() {
            return Ok(Self {
                version: 1,
                ..Default::default()
            });
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|e| LocusError::msg(format!("read onboard plan {}: {e}", path.display())))?;
        serde_json::from_str(&text).map_err(|e| LocusError::msg(format!("parse onboard plan: {e}")))
    }

    pub fn save(&self, home: &Path) -> Result<()> {
        let path = Self::plan_path(home);
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| LocusError::msg(format!("serialize onboard plan: {e}")))?;
        std::fs::write(&path, text)
            .map_err(|e| LocusError::msg(format!("write onboard plan: {e}")))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    pub fn reset(home: &Path) -> Result<()> {
        let path = Self::plan_path(home);
        if path.exists() {
            std::fs::remove_file(&path)
                .map_err(|e| LocusError::msg(format!("remove onboard plan: {e}")))?;
        }
        Ok(())
    }

    pub fn is_done(&self, step: &str) -> bool {
        self.completed.iter().any(|s| s == step)
    }

    pub fn mark_done(&mut self, step: &str) {
        if !self.is_done(step) {
            self.completed.push(step.to_string());
        }
    }

    pub fn candidate_key(c: &DetectedCandidate) -> String {
        format!("{}|{}|{}", c.source, c.provider, c.account_hint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp_home(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("locus-onboard-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn detect_env_vars_never_reads_values() {
        // Use a var name unlikely to collide; value content must not matter.
        std::env::set_var("LOCUS_ONBOARD_TEST_PROBE_XYZ", "x");
        let home = tmp_home("env");
        let cands = detect_ambient_candidates(&home, &home);
        // Our probe var is not in the known list → no candidate; the point is
        // no panic and no secret-shaped output.
        assert!(cands.iter().all(|c| !c.detail.contains("super-secret")));
        std::env::remove_var("LOCUS_ONBOARD_TEST_PROBE_XYZ");
    }

    #[test]
    fn detect_aws_config_profiles() {
        let home = tmp_home("aws");
        std::fs::create_dir_all(home.join(".aws")).unwrap();
        let mut f = std::fs::File::create(home.join(".aws").join("config")).unwrap();
        writeln!(
            f,
            "[profile acme-prod]\nregion = us-east-1\n[default]\nregion = us-west-2"
        )
        .unwrap();
        let cands = detect_ambient_candidates(&home, &home);
        let aws: Vec<_> = cands.iter().filter(|c| c.source == "aws-config").collect();
        assert!(aws.iter().any(|c| c.account_hint == "acme-prod"));
        assert!(aws.iter().any(|c| c.account_hint == "default"));
    }

    #[test]
    fn detect_mcp_json_servers() {
        let home = tmp_home("mcp");
        std::fs::write(
            home.join(".mcp.json"),
            r#"{"mcpServers": {"linear": {"command": "npx"}, "notion": {}}}"#,
        )
        .unwrap();
        let cands = detect_ambient_candidates(&home, &home);
        let names: Vec<_> = cands
            .iter()
            .filter(|c| c.source == "mcp-json")
            .map(|c| c.account_hint.as_str())
            .collect();
        assert!(names.contains(&"linear"));
        assert!(names.contains(&"notion"));
        // Malformed JSON → no candidates, no error.
        std::fs::write(home.join(".mcp.json"), "{oops").unwrap();
        let cands2 = detect_ambient_candidates(&home, &home);
        assert!(cands2
            .iter()
            .all(|c| c.source != "mcp-json" || !c.detail.contains("linear")));
    }

    #[test]
    fn detect_dedupes() {
        let home = tmp_home("dedupe");
        std::fs::create_dir_all(home.join(".aws")).unwrap();
        std::fs::write(
            home.join(".aws").join("config"),
            "[profile dup]\n[profile dup]\n",
        )
        .unwrap();
        let cands = detect_ambient_candidates(&home, &home);
        let dups: Vec<_> = cands
            .iter()
            .filter(|c| c.source == "aws-config" && c.account_hint == "dup")
            .collect();
        assert_eq!(dups.len(), 1);
    }

    #[test]
    fn detect_missing_everything_is_empty_ok() {
        let home = tmp_home("empty");
        // No .aws, no .ashlr, no .mcp.json; env probes depend on ambient env,
        // so only assert the call succeeds and candidates are well-formed.
        let cands = detect_ambient_candidates(&home, &home);
        for c in &cands {
            assert!(!c.source.is_empty() && !c.provider.is_empty());
        }
    }

    #[test]
    fn suggest_helpers() {
        assert_eq!(
            suggest_credential_ref("github", "acme-corp"),
            "env:GITHUB_ACME_CORP"
        );
        assert_eq!(suggest_alias("github", "octocat"), "github-octocat");
        assert_eq!(suggest_alias("aws", ""), "aws");
        assert_eq!(scope_fields_for_provider("supabase"), &["project_ref"]);
        assert!(scope_fields_for_provider("notion").is_empty());
    }

    #[test]
    fn planned_binding_rejects_raw_secrets() {
        let base = PlannedBinding {
            alias: "x".into(),
            tenant: "x".into(),
            provider: "github".into(),
            account: "acme".into(),
            credential_ref: "ghp_rawtokenvalue".into(),
            read_only: false,
            project_ref: None,
            team_id: None,
            account_id: None,
            org: None,
            repos: vec![],
            description: None,
        };
        assert!(base.validate().is_err());
        let ok = PlannedBinding {
            credential_ref: "phm:GH_ACME".into(),
            ..base.clone()
        };
        assert!(ok.validate().is_ok());
        let env_ok = PlannedBinding {
            credential_ref: "env:GH_TOKEN".into(),
            ..base
        };
        assert!(env_ok.validate().is_ok());
    }

    #[test]
    fn planned_binding_into_binding_round_trip() {
        let p = PlannedBinding {
            alias: "acme".into(),
            tenant: "Acme Corp".into(),
            provider: "github".into(),
            account: "acme".into(),
            credential_ref: "phm:GH_ACME".into(),
            read_only: true,
            project_ref: None,
            team_id: None,
            account_id: None,
            org: Some("acme".into()),
            repos: vec!["api".into()],
            description: None,
        };
        let b = p.into_binding().unwrap();
        assert_eq!(b.alias, "acme");
        assert_eq!(b.providers.len(), 1);
        assert_eq!(b.providers[0].credential_ref, "phm:GH_ACME");
        assert_eq!(b.providers[0].scope.read_only, Some(true));
        assert_eq!(b.providers[0].scope.orgs, vec!["acme"]);
    }

    #[test]
    fn plan_save_load_round_trip_and_resume() {
        let home = tmp_home("plan");
        let mut plan = OnboardPlan::load(&home).unwrap();
        assert!(!plan.is_done(steps::DETECT));
        plan.mark_done(steps::DETECT);
        plan.selected.push("gh|github|octocat".into());
        plan.save(&home).unwrap();
        // Refs only: the file must not contain anything secret-shaped.
        let text = std::fs::read_to_string(OnboardPlan::plan_path(&home)).unwrap();
        assert!(!text.contains("ghp_") && !text.contains("sk-"));
        let reloaded = OnboardPlan::load(&home).unwrap();
        assert!(reloaded.is_done(steps::DETECT));
        assert!(!reloaded.is_done(steps::TENANTS));
        assert_eq!(reloaded.selected, vec!["gh|github|octocat"]);
        OnboardPlan::reset(&home).unwrap();
        assert!(!OnboardPlan::plan_path(&home).exists());
    }

    #[test]
    fn candidate_key_stable() {
        let c = DetectedCandidate::new("gh", "github", "octocat", "d");
        assert_eq!(OnboardPlan::candidate_key(&c), "gh|github|octocat");
    }
    #[cfg(unix)]
    #[test]
    fn hung_gh_probe_returns_before_deadline_without_auth_access() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let gh = home.path().join("gh");
        std::fs::write(&gh, "#!/bin/sh\nsleep 10\n").unwrap();
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o700)).unwrap();
        let started = std::time::Instant::now();
        assert!(run_probe(gh.to_str().unwrap(), &["auth", "status"]).is_none());
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[test]
    fn gh_probe_reads_both_streams_once_and_only_emits_account_labels() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let gh = home.path().join("gh");
        let marker = home.path().join("calls");
        std::fs::write(&gh, format!("#!/bin/sh\nprintf x >> '{}'\nprintf 'Logged in to github.com account stdout-user (keyring)\\n'\nprintf 'Logged in to github.com account stderr-user (keyring)\\n' >&2\nprintf 'TOKEN: DO_NOT_DISCLOSE_CANARY\\n' >&2\n", marker.display())).unwrap();
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut candidates = Vec::new();
        detect_gh_cli_using(&mut candidates, gh.to_str().unwrap());
        assert_eq!(std::fs::read_to_string(marker).unwrap(), "x");
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].account_hint, "stdout-user");
        assert_eq!(candidates[1].account_hint, "stderr-user");
        assert!(!serde_json::to_string(&candidates)
            .unwrap()
            .contains("DO_NOT_DISCLOSE_CANARY"));
    }
    #[cfg(unix)]
    #[test]
    fn exited_gh_leader_does_not_leave_a_pipe_holding_child_alive() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let gh = home.path().join("gh");
        let pidfile = home.path().join("child-pid");
        std::fs::write(
            &gh,
            format!(
                "#!/bin/sh\nsleep 30 &\nprintf '%s' \"$!\" > '{}'\nexit 0\n",
                pidfile.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(run_probe(gh.to_str().unwrap(), &["auth", "status"]).is_none());
        let pid = std::fs::read_to_string(pidfile)
            .unwrap()
            .parse::<i32>()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            if unsafe { libc::kill(pid, 0) } != 0 {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("owned probe descendant survived group cleanup");
    }
}
