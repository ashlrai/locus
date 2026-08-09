//! Policy evaluation for tool calls.
//!
//! # Evaluation order
//!
//! 1. **Structured rules** (`[[binding.policy.rules]]`) — first matching rule wins
//! 2. **Legacy globs** — `require_approval`, then `dual_control`
//! 3. **`policy.default`** — `"allow"` or `"deny"`
//!
//! # Glob syntax (simplified)
//!
//! - `*` matches any run of characters (greedy across the whole name)
//! - `?` matches a single character
//! - matching is case-sensitive on the tool name string
//!
//! See [docs/policy.md](../../../docs/policy.md).

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
///
/// Order: structured rules (first match) → legacy require_approval/dual_control → default.
pub fn evaluate(policy: &Policy, tool: &str) -> PolicyVerdict {
    // 1) Structured rules — first matching rule wins
    for rule in &policy.rules {
        if !glob_match(&rule.match_glob, tool) {
            continue;
        }
        let action = rule.action.trim();
        return match action.to_ascii_lowercase().as_str() {
            "allow" => PolicyVerdict {
                decision: Decision::Allow,
                matched_rule: Some(rule.match_glob.clone()),
                reason: format!(
                    "tool '{tool}' matched rule match='{}' action=allow",
                    rule.match_glob
                ),
            },
            "deny" => PolicyVerdict {
                decision: Decision::Deny,
                matched_rule: Some(rule.match_glob.clone()),
                reason: format!(
                    "tool '{tool}' matched rule match='{}' action=deny",
                    rule.match_glob
                ),
            },
            "require_approval" => {
                let dual = policy.requires_dual_control(tool);
                PolicyVerdict {
                    decision: Decision::RequireApproval,
                    matched_rule: Some(rule.match_glob.clone()),
                    reason: if dual {
                        format!(
                            "tool '{tool}' matched rule match='{}' action=require_approval (dual_control)",
                            rule.match_glob
                        )
                    } else {
                        format!(
                            "tool '{tool}' matched rule match='{}' action=require_approval",
                            rule.match_glob
                        )
                    },
                }
            }
            "dual_control" => PolicyVerdict {
                decision: Decision::RequireApproval,
                matched_rule: Some(rule.match_glob.clone()),
                reason: format!(
                    "tool '{tool}' matched rule match='{}' action=dual_control",
                    rule.match_glob
                ),
            },
            other => PolicyVerdict {
                // Unknown action: fail closed
                decision: Decision::Deny,
                matched_rule: Some(rule.match_glob.clone()),
                reason: format!(
                    "tool '{tool}' matched rule match='{}' with unknown action '{other}' (fail closed)",
                    rule.match_glob
                ),
            },
        };
    }

    // 2) Legacy require_approval globs
    for pat in &policy.require_approval {
        if glob_match(pat, tool) {
            let dual = policy.requires_dual_control(tool);
            return PolicyVerdict {
                decision: Decision::RequireApproval,
                matched_rule: Some(pat.clone()),
                reason: if dual {
                    format!("tool '{tool}' matches require_approval pattern '{pat}' (dual_control)")
                } else {
                    format!("tool '{tool}' matches require_approval pattern '{pat}'")
                },
            };
        }
    }

    // 3) Legacy dual_control globs also require approval even if not in require_approval
    for pat in &policy.dual_control {
        if glob_match(pat, tool) {
            return PolicyVerdict {
                decision: Decision::RequireApproval,
                matched_rule: Some(pat.clone()),
                reason: format!("tool '{tool}' matches dual_control pattern '{pat}'"),
            };
        }
    }

    // 4) Default
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

/// Simple glob: `*` = any chars (greedy), `?` = single char, no character classes.
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
    use crate::binding::{Policy, PolicyRule};

    fn empty_policy() -> Policy {
        Policy {
            default: "allow".into(),
            rules: vec![],
            require_approval: vec![],
            dual_control: vec![],
            dual_control_all_approvals: false,
            max_ttl: None,
            parallel_sessions: 4,
        }
    }

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
            require_approval: vec!["*.delete*".into(), "vercel.deploy.prod".into()],
            ..empty_policy()
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
    fn dual_control_implies_require_approval() {
        let p = Policy {
            dual_control: vec!["vercel.deploy.prod".into()],
            ..empty_policy()
        };
        let v = evaluate(&p, "vercel.deploy.prod");
        assert_eq!(v.decision, Decision::RequireApproval);
        assert!(v.reason.contains("dual_control"));
        assert!(p.requires_dual_control("vercel.deploy.prod"));
        assert!(!p.requires_dual_control("github.list_prs"));
    }

    #[test]
    fn dual_control_all_approvals() {
        let p = Policy {
            require_approval: vec!["*.delete*".into()],
            dual_control_all_approvals: true,
            ..empty_policy()
        };
        assert!(p.requires_dual_control("supabase.table.delete"));
        assert!(!p.requires_dual_control("github.list_prs"));
        let v = evaluate(&p, "supabase.table.delete");
        assert_eq!(v.decision, Decision::RequireApproval);
        assert!(v.reason.contains("dual_control"));
    }

    #[test]
    fn default_deny() {
        let p = Policy {
            default: "deny".into(),
            ..empty_policy()
        };
        assert_eq!(evaluate(&p, "anything").decision, Decision::Deny);
    }

    // ── Structured rules ──────────────────────────────────────────────────

    #[test]
    fn rules_first_match_wins_allow_before_require() {
        // More-specific allow must beat a later require_approval if listed first.
        let p = Policy {
            rules: vec![
                PolicyRule::new("supabase.list*", "allow"),
                PolicyRule::new("supabase.*", "require_approval"),
            ],
            ..empty_policy()
        };
        assert_eq!(
            evaluate(&p, "supabase.list_tables").decision,
            Decision::Allow
        );
        assert_eq!(
            evaluate(&p, "supabase.execute_sql").decision,
            Decision::RequireApproval
        );
    }

    #[test]
    fn rules_first_match_wins_deny_before_allow() {
        let p = Policy {
            rules: vec![
                PolicyRule::new("*.drop*", "deny"),
                PolicyRule::new("*", "allow"),
            ],
            default: "deny".into(),
            ..empty_policy()
        };
        assert_eq!(evaluate(&p, "supabase.table.drop").decision, Decision::Deny);
        assert_eq!(evaluate(&p, "github.list_prs").decision, Decision::Allow);
    }

    #[test]
    fn rules_require_approval_action() {
        let p = Policy {
            rules: vec![PolicyRule::new("*.delete*", "require_approval")],
            ..empty_policy()
        };
        let v = evaluate(&p, "supabase.table.delete");
        assert_eq!(v.decision, Decision::RequireApproval);
        assert_eq!(v.matched_rule.as_deref(), Some("*.delete*"));
        assert!(v.reason.contains("action=require_approval"));
        assert_eq!(evaluate(&p, "github.list_prs").decision, Decision::Allow);
    }

    #[test]
    fn rules_dual_control_action() {
        let p = Policy {
            rules: vec![PolicyRule::new("vercel.deploy.prod", "dual_control")],
            ..empty_policy()
        };
        let v = evaluate(&p, "vercel.deploy.prod");
        assert_eq!(v.decision, Decision::RequireApproval);
        assert!(v.reason.contains("dual_control"));
        assert!(p.requires_dual_control("vercel.deploy.prod"));
        assert!(!p.requires_dual_control("vercel.deploy.preview"));
    }

    #[test]
    fn rules_before_legacy_globs() {
        // Structured allow beats legacy require_approval for the same tool.
        let p = Policy {
            rules: vec![PolicyRule::new("supabase.table.delete", "allow")],
            require_approval: vec!["*.delete*".into()],
            ..empty_policy()
        };
        assert_eq!(
            evaluate(&p, "supabase.table.delete").decision,
            Decision::Allow
        );
        // Other deletes still hit legacy
        assert_eq!(
            evaluate(&p, "github.repo.delete").decision,
            Decision::RequireApproval
        );
    }

    #[test]
    fn rules_no_match_falls_through_to_legacy_then_default() {
        let p = Policy {
            rules: vec![PolicyRule::new("stripe.*", "deny")],
            require_approval: vec!["*.delete*".into()],
            dual_control: vec!["vercel.deploy.prod".into()],
            default: "allow".into(),
            ..empty_policy()
        };
        // rule
        assert_eq!(evaluate(&p, "stripe.charge").decision, Decision::Deny);
        // legacy require_approval
        assert_eq!(
            evaluate(&p, "supabase.table.delete").decision,
            Decision::RequireApproval
        );
        // legacy dual_control
        assert_eq!(
            evaluate(&p, "vercel.deploy.prod").decision,
            Decision::RequireApproval
        );
        // default
        assert_eq!(evaluate(&p, "github.list_prs").decision, Decision::Allow);
    }

    #[test]
    fn rules_unknown_action_fail_closed() {
        let p = Policy {
            rules: vec![PolicyRule::new("evil.*", "explode")],
            default: "allow".into(),
            ..empty_policy()
        };
        assert_eq!(evaluate(&p, "evil.thing").decision, Decision::Deny);
        assert!(evaluate(&p, "evil.thing").reason.contains("unknown action"));
    }

    #[test]
    fn dual_control_all_applies_to_structured_require_approval() {
        let p = Policy {
            rules: vec![PolicyRule::new("*.delete*", "require_approval")],
            dual_control_all_approvals: true,
            ..empty_policy()
        };
        assert!(p.requires_dual_control("supabase.table.delete"));
        let v = evaluate(&p, "supabase.table.delete");
        assert_eq!(v.decision, Decision::RequireApproval);
        assert!(v.reason.contains("dual_control"));
    }

    #[test]
    fn rules_toml_roundtrip_match_field() {
        let toml_src = r#"
default = "allow"
[[rules]]
match = "supabase.*"
action = "allow"
[[rules]]
match = "*.delete*"
action = "require_approval"
[[rules]]
match = "vercel.deploy.prod"
action = "dual_control"
"#;
        let p: Policy = toml::from_str(toml_src).expect("parse policy with rules");
        assert_eq!(p.rules.len(), 3);
        assert_eq!(p.rules[0].match_glob, "supabase.*");
        assert_eq!(p.rules[0].action, "allow");
        assert_eq!(
            evaluate(&p, "supabase.execute_sql").decision,
            Decision::Allow
        );
        assert_eq!(
            evaluate(&p, "github.repo.delete").decision,
            Decision::RequireApproval
        );
        assert!(p.requires_dual_control("vercel.deploy.prod"));
    }

    #[test]
    fn rules_ordering_delete_after_provider_allow() {
        // Common firm pattern: allow read surface, gate deletes, dual prod deploy.
        let p = Policy {
            rules: vec![
                PolicyRule::new("supabase.select*", "allow"),
                PolicyRule::new("*.delete*", "require_approval"),
                PolicyRule::new("vercel.deploy.prod", "dual_control"),
            ],
            default: "allow".into(),
            ..empty_policy()
        };
        assert_eq!(
            evaluate(&p, "supabase.select_rows").decision,
            Decision::Allow
        );
        assert_eq!(
            evaluate(&p, "supabase.table.delete").decision,
            Decision::RequireApproval
        );
        assert!(!p.requires_dual_control("supabase.table.delete"));
        let prod = evaluate(&p, "vercel.deploy.prod");
        assert_eq!(prod.decision, Decision::RequireApproval);
        assert!(p.requires_dual_control("vercel.deploy.prod"));
    }
}
