//! Shared first-run templates. Exact semantic matching limits sample-only
//! diagnostics to bindings whose policy, scopes, refs and providers are unchanged.

pub(crate) const PERSONAL: &str = r#"# Sample personal binding — REPLACE project_ref / team_id placeholders.
# CredentialRefs are explicit env:VAR pointers. Never put raw tokens here.
# Supply those variables outside agent context before supervised execution.
# phm: references currently require an unavailable scoped bridge.
# Then: locus enter personal && locus whoami && locus doctor

[binding]
id = "bnd_personal"
alias = "personal"
tenant = "personal"
description = "Personal projects — sample; edit scopes before real use"

[binding.policy]
default = "allow"
max_ttl = "12h"

[[binding.providers]]
provider = "supabase"
account = "personal"
credential_ref = "env:LOCUS_SUPABASE_PERSONAL"
# project_ref is frozen on every tool call — set the real ref.
scope = { project_ref = "personal_ref_replace_me", read_only = false }

[[binding.providers]]
provider = "github"
account = "personal"
credential_ref = "env:LOCUS_GH_TOKEN_PERSONAL"
# Empty orgs/repos = no allowlist restriction (tighten for client work).
scope = { orgs = [], repos = [] }

[[binding.providers]]
provider = "vercel"
account = "personal"
credential_ref = "env:LOCUS_VERCEL_TOKEN_PERSONAL"
scope = { team_id = "team_personal_replace_me", env = ["preview", "production"] }
"#;

pub(crate) const ACME: &str = r#"# Sample client binding (Acme) — REPLACE placeholders before real work.
# Client work: prefer read_only on prod Supabase; require_approval on delete/deploy.
# Switch: locus switch acme   |   leave: locus leave
# Dual-control / firm mode: docs/firm-mode.md

[binding]
id = "bnd_acme"
alias = "acme"
tenant = "acme-corp"
description = "Acme client engagement — sample"

[binding.policy]
default = "allow"
max_ttl = "8h"
# Closed external authorization is required; local approve labels are advisory only.
require_approval = ["*.delete*", "vercel.deploy.prod"]

[[binding.providers]]
provider = "supabase"
account = "acme-prod"
credential_ref = "env:LOCUS_SUPABASE_ACME"
scope = { project_ref = "acme_ref_replace_me", read_only = true }

[[binding.providers]]
provider = "github"
account = "acme-corp"
credential_ref = "env:LOCUS_GH_TOKEN_ACME"
# Frozen org/repo allowlist — model cannot reach outside.
scope = { orgs = ["acme-corp"], repos = ["acme-corp/*"] }

[[binding.providers]]
provider = "vercel"
account = "acme-team"
credential_ref = "env:LOCUS_VERCEL_TOKEN_ACME"
scope = { team_id = "team_acme_replace_me", projects = ["acme-web"], env = ["preview"] }
"#;

pub(crate) fn is_sample(binding: &locus_core::Binding) -> bool {
    [PERSONAL, ACME]
        .iter()
        .any(|raw| locus_core::Binding::parse_toml(raw).is_ok_and(|sample| sample == *binding))
}
