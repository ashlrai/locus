//! Policy evaluation for tool calls.
//!
//! Glob syntax (simplified):
//! - `*` matches any run of characters except `.` path separators in tool names
//! - `**` / bare `*` at ends matches greedily across the whole name
//! - matching is case-sensitive on the tool name string

use crate::binding::Policy;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Allow,
    Deny,
    RequireApproval,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyVerdict {
    pub decision: Decision,
    pub matched_rule: Option<String>,
    pub reason: String,
}

/// Evaluate policy for a tool name (e.g. `supabase.execute_sql`, `vercel.deploy.prod`).
pub fn evaluate(policy: &Policy, tool: &str) -> PolicyVerdict {
    // require_approval first (highest priority among affirmative controls)
    for pat in &policy.require_approval {
        if glob_match(pat, tool) {
            return PolicyVerdict {
                decision: Decision::RequireApproval,
                matched_rule: Some(pat.clone()),
                reason: format!("tool '{tool}' matches require_approval pattern '{pat}'"),
            };
        }
    }

    match policy.default.as_str() {
        "deny" => PolicyVerdict {
            decision: Decision::Deny,
            matched_rule: Some("default".into()),
            reason: "policy.default = deny".into(),
        },
        _ => PolicyVerdict {
            decision: Decision::Allow,
            matched_rule: Some("default".into()),
            reason: "policy.default = allow".into(),
        },
    }
}

/// Simple glob: `*` = any chars (greedy), no character classes.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    glob_match_inner(pattern.as_bytes(), text.as_bytes())
}

fn glob_match_inner(pat: &[u8], text: &[u8]) -> bool {
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star_pi = None;
    let mut star_ti = 0usize;

    while ti < text.len() {
        if pi < pat.len() && (pat[pi] == text[ti] || pat[pi] == b'?') {
            pi += 1;
            ti += 1;
        } else if pi < pat.len() && pat[pi] == b'*' {
            star_pi = Some(pi);
            star_ti = ti;
            pi += 1;
        } else if let Some(sp) = star_pi {
            pi = sp + 1;
            star_ti += 1;
            ti = star_ti;
        } else {
            return false;
        }
    }
    while pi < pat.len() && pat[pi] == b'*' {
        pi += 1;
    }
    pi == pat.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::Policy;

    #[test]
    fn glob_basics() {
        assert!(glob_match("*.delete*", "supabase.table.delete"));
        assert!(glob_match("vercel.deploy.prod", "vercel.deploy.prod"));
        assert!(!glob_match("vercel.deploy.prod", "vercel.deploy.preview"));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("supabase.*", "supabase.execute_sql"));
        assert!(!glob_match("supabase.*", "github.list_prs"));
    }

    #[test]
    fn require_approval_priority() {
        let p = Policy {
            default: "allow".into(),
            require_approval: vec!["*.delete*".into(), "vercel.deploy.prod".into()],
            max_ttl: None,
            parallel_sessions: 4,
        };
        assert_eq!(
            evaluate(&p, "supabase.table.delete").decision,
            Decision::RequireApproval
        );
        assert_eq!(
            evaluate(&p, "vercel.deploy.prod").decision,
            Decision::RequireApproval
        );
        assert_eq!(evaluate(&p, "github.list_prs").decision, Decision::Allow);
    }

    #[test]
    fn default_deny() {
        let p = Policy {
            default: "deny".into(),
            require_approval: vec![],
            max_ttl: None,
            parallel_sessions: 1,
        };
        assert_eq!(evaluate(&p, "anything").decision, Decision::Deny);
    }
}
