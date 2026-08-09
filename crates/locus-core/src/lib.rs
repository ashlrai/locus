//! Locus core — identity plane primitives for multi-account agents.
//!
//! # Invariants
//!
//! - A session is sealed to exactly one Binding (exclusive mode).
//! - Unbound sessions have no provider surface.
//! - Isolated exec env contains only the pinned binding's providers.
//! - Credential **values** never live in MCP responses — only CredentialRefs
//!   and resolved secrets inside worker env maps.

pub mod adapters;
pub mod approval;
pub mod autopin;
pub mod binding;
pub mod config;
pub mod credential;
pub mod doctor;
pub mod engagement;
pub mod error;
pub mod isolation;
pub mod policy;
pub mod seal;
pub mod session;
pub mod store;
pub mod workers;
pub mod workspace;

pub use adapters::{
    call_tool, call_tool_gated, control_tools, enforce_policy, tools_for_binding, AdapterTool,
    ApprovalGate, ToolCallResult,
};
pub use approval::notifications_enabled;
pub use approval::{
    args_digest, default_grant_ttl, mint_approval_id, required_grant_count, validate_approval_id,
    ApprovalGrant, ApprovalRecord, ApprovalStatus,
};
pub use autopin::{match_remote_binding, resolve_auto_pin, AutoPinTarget};
pub use binding::{
    validate_name_component, Binding, BindingBody, BindingSummary, Policy, PolicyRule,
    ProviderBinding, Scope, UpstreamSpec,
};
pub use config::{
    load_config, save_config, AutopinConfig, AutopinRemote, AutopinStatus, LocusConfig,
    NotifyConfig,
};
pub use credential::{inject_keys_for_provider, resolve, resolve_binding_secrets, CredentialRef};
pub use doctor::{
    build_doctor_report, doctor_json_has_stable_keys, filter_audit_events, AuditSummary,
    DoctorExternal, DoctorIssue, DoctorPin, DoctorReport, DoctorVerdict, IssueSeverity,
    WorkspaceStatus as DoctorWorkspaceStatus, DOCTOR_JSON_KEYS,
};
pub use engagement::{
    client_binding_template, close_checklist, engagement_readme, EngagementCloseResult,
    EngagementMeta,
};
pub use error::{LocusError, Result};
pub use isolation::{
    build_isolated_env, build_isolated_env_opts, build_isolated_env_strict,
    visible_credential_refs, IsolatedEnv,
};
pub use policy::{evaluate as evaluate_policy, glob_match, Decision, PolicyVerdict};
pub use seal::SealKey;
pub use session::{
    binding_fingerprint, namespace_tool, parse_ttl, split_namespaced_tool, PinSource, Session,
    SessionMode,
};
pub use store::{
    locus_home, ApprovalsHealth, AuditEvent, EngagementInitResult, ProviderView, RuntimeDrift,
    Store, Whoami,
};
pub use workers::{
    mcp_config_from_upstream, namespace_upstream_tool, provider_from_tool_name,
    strip_provider_prefix, CompositeWorkerManager, InMemoryWorkerManager, McpStdioBackend,
    McpStdioClient, McpStdioConfig, SyntheticBackend, UpstreamTool, WorkerBackend, WorkerKey,
    WorkerManager, WorkerSlot, WorkerState, WorkerToolResult,
};
pub use workspace::{find_workspace, WorkspaceConfig};

/// Crate version for `locus --version` / doctor.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
