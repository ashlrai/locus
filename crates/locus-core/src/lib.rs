//! Locus core — identity plane primitives for multi-account agents.
//!
//! # Invariants
//!
//! - A session is sealed to exactly one Binding (exclusive mode).
//! - Unbound sessions have no provider surface.
//! - Isolated exec env contains only the pinned binding's providers.
//! - Credential refs and values never live in MCP responses; only safe source
//!   metadata is exposed, and resolved values stay inside worker env maps.

pub mod adapter_registry;
pub mod adapter_trust;
pub mod adapters;
pub mod agent_report;
pub mod approval;
mod authority_anchor;
pub mod autopin;
pub mod binding;
pub mod config;
pub mod credential;
mod credential_migration;
pub mod doctor;
pub mod engagement;
pub mod error;
pub mod events;
pub mod forensics;
pub mod graph;
pub mod http_sessions;
pub mod isolation;
pub mod policy;
pub mod recipes;
pub mod seal;
pub mod session;
pub mod store;
pub mod ticket;
pub mod verify;
pub mod workers;
pub mod workspace;

pub use adapter_registry::{
    builtin_manifest, canonical_entry_material, ed25519_public_key_b64, list_adapters,
    parse_manifest, parse_trust_keys_env, sign_entry, sign_entry_ed25519, sign_entry_material,
    sign_entry_material_ed25519, verify_builtin, verify_entry, verify_entry_with_keys,
    verify_manifest, verify_manifest_with_keys, AdapterManifest, AdapterManifestEntry,
    EntryVerifyReport, EntryVerifyStatus, ManifestVerifyReport, RegistryKeyMaterial,
    RegistryTrustKey, LOCUS_ADAPTER_TRUST_KEYS_ENV, SIG_SCHEME_ED25519, SIG_SCHEME_HMAC_SHA256,
};
pub use adapter_trust::{
    adapter_trust_dir, adapter_trust_keys_path, add_ed25519_trust_key, add_trust_key,
    list_trust_keys_file, list_trust_keys_with_origin, load_merged_trust_keys,
    load_merged_trust_keys_default, load_trust_keys_file, merge_trust_keys, parse_trust_keys_file,
    save_trust_keys_file, AdapterTrustKeyFileEntry, AdapterTrustKeysFile, TrustKeyAddResult,
    TrustKeyListing, TrustKeyOrigin, ADAPTER_TRUST_DIR_NAME, ADAPTER_TRUST_KEYS_FILE_NAME,
    ADAPTER_TRUST_KEYS_VERSION,
};
pub use adapters::{
    call_tool, call_tool_gated, control_tools, enforce_policy, known_providers, tools_for_binding,
    AdapterTool, ApprovalGate, ToolCallResult,
};
pub use agent_report::{
    agent_md_content, agent_md_path, agent_md_present, agent_report_from_doctor,
    agent_report_json_has_stable_keys, build_agent_report, compute_safe_next, mcp_agent_env,
    probe_agent_options, probe_mcp_registered, probe_mcp_registered_with_grok, verify_session,
    workspace_present, workspace_stub_toml, AgentCommands, AgentReport, AgentReportOptions,
    AgentStatus, ClaudeMcpScope, McpRegistered, SafeNext, SessionVerificationPack,
    AGENT_REPORT_JSON_KEYS, REQUIRED_SERVERS,
};
pub use approval::notifications_enabled;
pub use approval::{
    agent_approval_hint, args_digest, default_grant_ttl, external_approval_authority_enabled,
    format_dual_control_progress, format_grants_progress, mint_approval_id, next_grant_command,
    notification_body, partial_grant_notification_body, required_grant_count, validate_approval_id,
    ApprovalAuthority, ApprovalGrant, ApprovalRecord, ApprovalStatus, ExternalApprovalEnvelope,
    EXTERNAL_APPROVAL_AUTHORITY_BLOCKER,
};
pub use authority_anchor::{
    control_capability_file, control_capability_status, mint_ephemeral_control_capability,
    mint_persisted_control_capability, persist_control_capability,
    read_persisted_control_capability, restrict_validation_to_executor,
    run_authority_anchor_server_if_requested, unpersist_control_capability,
    ControlCapabilityStatus, CONTROL_CAPABILITY_ENV, EXECUTOR_CAPABILITY_ENV,
};
pub use autopin::{match_remote_binding, resolve_auto_pin, AutoPinTarget};
pub use binding::{
    validate_name_component, Binding, BindingBody, BindingSummary, Policy, PolicyRule,
    ProviderBinding, Scope, UpstreamSpec,
};
pub use config::{
    load_config, AutopinConfig, AutopinRemote, AutopinStatus, LocusConfig, NotifyConfig,
};
pub use credential::{
    collect_unresolved_phm_refs, credential_metadata, inject_keys_for_provider,
    migrate_legacy_phantom_ref, phantom_on_path, resolve, resolve_binding_secrets,
    CredentialMetadata, CredentialRef, CredentialResolutionIssue, ResolvedBindingSecrets,
};
pub use doctor::{
    build_doctor_report, control_capability_findings, count_near_misses,
    doctor_json_has_stable_keys, filter_audit_events, gather_doctor_external, is_near_miss_op,
    AuditSummary, DoctorExternal, DoctorIssue, DoctorPin, DoctorReport, DoctorVerdict,
    IssueSeverity, McpMultiTenantHealth, NearMissSummary, WorkspaceStatus as DoctorWorkspaceStatus,
    DOCTOR_JSON_KEYS,
};
pub use engagement::{
    client_binding_template, close_checklist, engagement_readme, EngagementCloseResult,
    EngagementMeta,
};
pub use error::{LocusError, Result};
pub use events::{
    assert_export_body_no_secrets, export_body_secret_issues, export_content_type, export_events,
    post_audit_webhook, resolve_audit_webhook_url, webhook_url_safe_label, EventsExportFormat,
    EventsExportOptions, EventsExportSink, FleetPulseEvent, WebhookPostResult,
    AUDIT_WEBHOOK_TIMEOUT_SECS, AUDIT_WEBHOOK_URL_ENV,
};
pub use forensics::{
    export_forensics_pack, forensics_pack_json_has_stable_keys, AuditChainTip,
    ForensicsExportOptions, ForensicsPack, DEFAULT_AUDIT_LAST, FORENSICS_PACK_JSON_KEYS,
};
pub use graph::{
    decrypt_graph, default_export_filename, encrypt_graph, resolve_passphrase, source_host,
    GraphEnvelope, GraphExportResult, GraphImportResult, GraphListEntry, GraphMeta,
    WorkspaceTemplate, ENV_PASSPHRASE as GRAPH_PASSPHRASE_ENV, GRAPH_VERSION, MAGIC as GRAPH_MAGIC,
};
pub use isolation::{
    build_ci_env_map, build_isolated_env, build_isolated_env_for_provider_opts,
    build_isolated_env_opts, build_isolated_env_strict, ci_secrets_allowed, IsolatedEnv,
};
pub use policy::{evaluate as evaluate_policy, glob_match, Decision, PolicyVerdict};
pub use recipes::{
    all_recipes, get_recipe, recipe_toml_snippet, suggest_for_provider, RecipeReadiness,
    SandboxCompatibility, UpstreamRecipe,
};
pub use seal::SealKey;
pub use session::{
    binding_fingerprint, namespace_tool, parse_ttl, split_namespaced_tool, PinSource, Session,
    SessionAuthority, SessionAuthorityAnchor, SessionBackingIdentity, SessionBackingType,
    SessionMode, CURRENT_SEAL_VERSION,
};
pub use store::{
    is_safe_mcp_grant_id, locus_home, parse_mcp_grant_token, ApprovalsHealth, AuditEvent,
    CredentialRefMigration, EngagementInitResult, ForceLeaveOutcome, McpGrant, McpGrantAuthError,
    ProviderView, ResolvedSession, RuntimeDrift, Store, Whoami,
};
pub use ticket::{
    mint_ticket, verify_ticket, verify_ticket_parts, CapabilityTicket, DEFAULT_TICKET_TTL_SECS,
    TICKET_ID_PREFIX,
};
pub use verify::{
    count_low_confidence_audit_signals, doctor_low_confidence_message, verify_claim,
    ClaimConfidence, ClaimGrounding, ClaimVerification, DOCTOR_LOW_CONFIDENCE_AUDIT_SCAN,
    DOCTOR_LOW_CONFIDENCE_AUDIT_THRESHOLD,
};
pub use workers::{
    idle_timeout_from_env, mcp_config_from_upstream, namespace_upstream_tool,
    provider_from_tool_name, restricted_worker_path, sandbox_enabled, sandbox_from_env,
    sandbox_no_network_enabled, sandbox_no_network_from_env, strip_provider_prefix,
    CompositeWorkerManager, InMemoryWorkerManager, McpStdioBackend, McpStdioClient, McpStdioConfig,
    SyntheticBackend, UpstreamTool, WorkerBackend, WorkerKey, WorkerManager, WorkerSlot,
    WorkerState, WorkerToolResult, ENV_WORKER_IDLE_SECS, ENV_WORKER_SANDBOX, ENV_WORKER_SANDBOXED,
    ENV_WORKER_SANDBOX_BACKEND, ENV_WORKER_SANDBOX_NO_NETWORK,
};
pub use workspace::{find_workspace, WorkspaceConfig};

/// Crate version for `locus --version` / doctor.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
