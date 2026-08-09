//! Opt-in auto-pin heuristics from `$LOCUS_HOME/config.toml` `[autopin]`.
//!
//! Resolution order for `locus pin` / `locus enter` with no alias:
//! 1. Workspace `.locus.toml` `default_binding`
//! 2. Git remote URL substring match (only when `autopin.enabled = true`)
//!
//! Never auto-pins a binding blocked by the workspace allowlist.
//! Never uses `--force` for autopin matches.

use crate::config::{load_config, AutopinRemote};
use crate::error::{LocusError, Result};
use crate::session::PinSource;
use crate::workspace::{find_workspace, WorkspaceConfig};
use std::path::Path;
use std::process::Command;

/// Resolved auto-pin target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoPinTarget {
    pub alias: String,
    pub source: PinSource,
    /// Human-readable reason: `workspace_default` | `git_remote`.
    pub reason: String,
}

/// Collect remote URLs from `git -C <cwd> remote -v` (best-effort).
///
/// Returns empty if git is missing, cwd is not a repo, or the command fails.
pub fn git_remote_urls(cwd: &Path) -> Vec<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["remote", "-v"])
        .output();
    let Ok(out) = output else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut urls = Vec::new();
    for line in stdout.lines() {
        // origin  git@github.com:acme/repo.git (fetch)
        let mut parts = line.split_whitespace();
        let _name = parts.next();
        if let Some(url) = parts.next() {
            if !urls.iter().any(|u: &String| u == url) {
                urls.push(url.to_string());
            }
        }
    }
    urls
}

/// Normalize git remote URLs so SSH and HTTPS forms share a comparable shape.
///
/// - `git@github.com:acme-corp/app.git` → `github.com/acme-corp/app.git`
/// - `ssh://git@github.com/acme-corp/app.git` → `github.com/acme-corp/app.git`
/// - `https://github.com/acme-corp/app.git` → `github.com/acme-corp/app.git`
pub fn normalize_remote_url(url: &str) -> String {
    let mut s = url.trim().to_string();
    // strip protocol
    for prefix in ["https://", "http://", "ssh://", "git://"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.to_string();
            break;
        }
    }
    // git@host:path → host/path
    if let Some(at) = s.find('@') {
        let after_at = &s[at + 1..];
        if let Some(colon) = after_at.find(':') {
            // only treat as scp-like if no port-only confusion: host:path with /
            let host = &after_at[..colon];
            let path = &after_at[colon + 1..];
            if !host.is_empty() && path.contains('/') {
                s = format!("{host}/{path}");
            } else {
                s = after_at.to_string();
            }
        } else {
            s = after_at.to_string();
        }
    }
    // drop trailing .git
    if let Some(stripped) = s.strip_suffix(".git") {
        s = stripped.to_string();
    }
    s
}

/// Match git remotes against autopin rules; first hit wins.
///
/// Patterns are matched against both the raw URL and a normalized form so
/// `github.com/acme-corp` hits `git@github.com:acme-corp/…`.
pub fn match_remote_binding(urls: &[String], rules: &[AutopinRemote]) -> Option<String> {
    for rule in rules {
        if rule.match_pattern.is_empty() || rule.binding.is_empty() {
            continue;
        }
        let pat = rule.match_pattern.as_str();
        let pat_norm = normalize_remote_url(pat);
        for url in urls {
            let norm = normalize_remote_url(url);
            if url.contains(pat) || norm.contains(pat) || norm.contains(&pat_norm) {
                return Some(rule.binding.clone());
            }
        }
    }
    None
}

/// Whether the workspace allowlist permits this alias (empty allowlist = allow all).
pub fn alias_allowed_in_workspace(cfg: Option<&WorkspaceConfig>, alias: &str) -> bool {
    match cfg {
        None => true,
        Some(c) => c.allows(alias),
    }
}

/// Resolve bare `pin` / `enter` alias.
///
/// Order: workspace `default_binding`, then (if enabled) git remote match.
/// Git remote matches that fail the workspace allowlist are skipped (never force).
pub fn resolve_auto_pin(cwd: &Path, home: &Path) -> Result<AutoPinTarget> {
    let ws = find_workspace(cwd)?;
    let ws_cfg = ws.as_ref().map(|(_, c)| c);

    if let Some((ref path, ref cfg)) = ws {
        if let Some(ref alias) = cfg.default_binding {
            if !alias.is_empty() {
                return Ok(AutoPinTarget {
                    alias: alias.clone(),
                    source: PinSource::Dir {
                        path: path.display().to_string(),
                    },
                    reason: "workspace_default".into(),
                });
            }
        }
    }

    let config = load_config(home);
    if !config.autopin.enabled {
        return Err(LocusError::msg(
            "no binding specified and no default_binding in .locus.toml — try `locus pin <alias>` or `locus enter <alias>` (enable [autopin] in config.toml for git-remote matching)",
        ));
    }

    let urls = git_remote_urls(cwd);
    if urls.is_empty() {
        return Err(LocusError::msg(
            "autopin enabled but no git remotes found and no workspace default_binding — try `locus pin <alias>`",
        ));
    }

    // Walk rules in order; skip allowlist-blocked matches without forcing.
    for rule in &config.autopin.remotes {
        if rule.match_pattern.is_empty() || rule.binding.is_empty() {
            continue;
        }
        let hit = match_remote_binding(&urls, std::slice::from_ref(rule)).is_some();
        if !hit {
            continue;
        }
        if !alias_allowed_in_workspace(ws_cfg, &rule.binding) {
            continue;
        }
        return Ok(AutoPinTarget {
            alias: rule.binding.clone(),
            source: PinSource::Autopin {
                match_pattern: rule.match_pattern.clone(),
            },
            reason: "git_remote".into(),
        });
    }

    Err(LocusError::msg(
        "autopin enabled but no remote matched an allowed binding — try `locus pin <alias>` or add [[autopin.remotes]] in config.toml",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{save_config, AutopinConfig, LocusConfig};
    use std::fs;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn normalize_ssh_and_https() {
        assert_eq!(
            normalize_remote_url("git@github.com:acme-corp/app.git"),
            "github.com/acme-corp/app"
        );
        assert_eq!(
            normalize_remote_url("https://github.com/acme-corp/app.git"),
            "github.com/acme-corp/app"
        );
        assert_eq!(
            normalize_remote_url("ssh://git@github.com/acme-corp/app.git"),
            "github.com/acme-corp/app"
        );
    }

    #[test]
    fn match_remote_first_hit() {
        let rules = vec![
            AutopinRemote {
                match_pattern: "github.com/acme-corp".into(),
                binding: "acme".into(),
            },
            AutopinRemote {
                match_pattern: "github.com/other".into(),
                binding: "other".into(),
            },
        ];
        let urls = vec!["git@github.com:acme-corp/app.git".into()];
        assert_eq!(match_remote_binding(&urls, &rules).as_deref(), Some("acme"));
    }

    #[test]
    fn resolve_workspace_default_wins() {
        let dir = tempdir().unwrap();
        let home = dir.path().join("locus-home");
        fs::create_dir_all(&home).unwrap();
        let cfg = LocusConfig {
            autopin: AutopinConfig {
                enabled: true,
                remotes: vec![AutopinRemote {
                    match_pattern: "github.com/x".into(),
                    binding: "x".into(),
                }],
            },
            ..Default::default()
        };
        save_config(&home, &cfg).unwrap();

        let project = dir.path().join("proj");
        fs::create_dir_all(&project).unwrap();
        let mut f = fs::File::create(project.join(".locus.toml")).unwrap();
        writeln!(
            f,
            r#"
version = 1
default_binding = "acme"
allowed_bindings = ["acme"]
"#
        )
        .unwrap();

        let t = resolve_auto_pin(&project, &home).unwrap();
        assert_eq!(t.alias, "acme");
        assert_eq!(t.reason, "workspace_default");
        assert!(matches!(t.source, PinSource::Dir { .. }));
    }

    #[test]
    fn resolve_git_remote_when_enabled() {
        let dir = tempdir().unwrap();
        let home = dir.path().join("locus-home");
        fs::create_dir_all(&home).unwrap();
        let cfg = LocusConfig {
            autopin: AutopinConfig {
                enabled: true,
                remotes: vec![AutopinRemote {
                    match_pattern: "github.com/acme-corp".into(),
                    binding: "acme".into(),
                }],
            },
            ..Default::default()
        };
        save_config(&home, &cfg).unwrap();

        let project = dir.path().join("repo");
        fs::create_dir_all(&project).unwrap();
        let init = Command::new("git")
            .args(["init"])
            .current_dir(&project)
            .output()
            .expect("git init");
        assert!(init.status.success(), "git init failed");
        let add = Command::new("git")
            .args([
                "remote",
                "add",
                "origin",
                "git@github.com:acme-corp/app.git",
            ])
            .current_dir(&project)
            .output()
            .expect("git remote add");
        assert!(add.status.success(), "git remote add failed");

        let t = resolve_auto_pin(&project, &home).unwrap();
        assert_eq!(t.alias, "acme");
        assert_eq!(t.reason, "git_remote");
        assert!(matches!(t.source, PinSource::Autopin { .. }));
    }

    #[test]
    fn resolve_skips_allowlist_blocked_autopin() {
        let dir = tempdir().unwrap();
        let home = dir.path().join("locus-home");
        fs::create_dir_all(&home).unwrap();
        let cfg = LocusConfig {
            autopin: AutopinConfig {
                enabled: true,
                remotes: vec![AutopinRemote {
                    match_pattern: "github.com/acme-corp".into(),
                    binding: "acme".into(),
                }],
            },
            ..Default::default()
        };
        save_config(&home, &cfg).unwrap();

        let project = dir.path().join("repo");
        fs::create_dir_all(&project).unwrap();
        fs::write(
            project.join(".locus.toml"),
            r#"
version = 1
allowed_bindings = ["other"]
"#,
        )
        .unwrap();
        let _ = Command::new("git")
            .args(["init"])
            .current_dir(&project)
            .output();
        let _ = Command::new("git")
            .args([
                "remote",
                "add",
                "origin",
                "https://github.com/acme-corp/app.git",
            ])
            .current_dir(&project)
            .output();

        let err = resolve_auto_pin(&project, &home).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("no remote matched") || msg.contains("allowed"),
            "unexpected: {msg}"
        );
    }

    #[test]
    fn disabled_autopin_errors_without_workspace() {
        let dir = tempdir().unwrap();
        let home = dir.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let project = dir.path().join("p");
        fs::create_dir_all(&project).unwrap();
        let err = resolve_auto_pin(&project, &home).unwrap_err();
        assert!(err.to_string().contains("no binding specified"));
    }

    #[test]
    fn malformed_workspace_blocks_remote_autopin() {
        let dir = tempdir().unwrap();
        let home = dir.path().join("locus-home");
        fs::create_dir_all(&home).unwrap();
        let cfg = LocusConfig {
            autopin: AutopinConfig {
                enabled: true,
                remotes: vec![AutopinRemote {
                    match_pattern: "github.com/acme-corp".into(),
                    binding: "acme".into(),
                }],
            },
            ..Default::default()
        };
        save_config(&home, &cfg).unwrap();

        let project = dir.path().join("repo");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join(".locus.toml"), "allowed_bindings = [").unwrap();
        let init = Command::new("git")
            .args(["init"])
            .current_dir(&project)
            .output()
            .unwrap();
        assert!(init.status.success());
        let add = Command::new("git")
            .args([
                "remote",
                "add",
                "origin",
                "git@github.com:acme-corp/app.git",
            ])
            .current_dir(&project)
            .output()
            .unwrap();
        assert!(add.status.success());

        let err = resolve_auto_pin(&project, &home).unwrap_err().to_string();
        assert!(err.contains("workspace policy malformed"));
    }

    #[cfg(unix)]
    #[test]
    fn broken_workspace_link_blocks_remote_autopin() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().unwrap();
        let home = dir.path().join("locus-home");
        fs::create_dir_all(&home).unwrap();
        let cfg = LocusConfig {
            autopin: AutopinConfig {
                enabled: true,
                remotes: vec![AutopinRemote {
                    match_pattern: "github.com/acme-corp".into(),
                    binding: "acme".into(),
                }],
            },
            ..Default::default()
        };
        save_config(&home, &cfg).unwrap();
        let project = dir.path().join("repo");
        fs::create_dir_all(&project).unwrap();
        symlink("missing-policy.toml", project.join(".locus.toml")).unwrap();

        let error = resolve_auto_pin(&project, &home).unwrap_err().to_string();
        assert!(error.contains("broken or unreadable"), "{error}");
    }
}
