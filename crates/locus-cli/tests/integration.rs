//! CLI integration tests share one harness to avoid linking each test separately.

#[path = "agent_contract.rs"]
mod agent_contract;
#[path = "agent_contract_recut.rs"]
mod agent_contract_recut;
#[path = "agent_input_contracts.rs"]
mod agent_input_contracts;
#[path = "ashlr_handshake.rs"]
mod ashlr_handshake;
#[path = "autopin_hardening.rs"]
mod autopin_hardening;
#[path = "autopin_learned_guard.rs"]
mod autopin_learned_guard;
#[path = "autopin_suggest.rs"]
mod autopin_suggest;
#[path = "binding_add_idempotent.rs"]
mod binding_add_idempotent;
#[path = "coverage_home_isolation.rs"]
mod coverage_home_isolation;
#[path = "dead_broker_hint.rs"]
mod dead_broker_hint;
#[path = "doc_facts.rs"]
mod doc_facts;
#[path = "doctor_after_quickstart.rs"]
mod doctor_after_quickstart;
#[path = "doctor_fix_cli.rs"]
mod doctor_fix_cli;
#[path = "doctor_fix_phm_names.rs"]
mod doctor_fix_phm_names;
#[path = "dry_run_docs.rs"]
mod dry_run_docs;
#[path = "dry_run_docs_coverage.rs"]
mod dry_run_docs_coverage;
#[path = "dry_run_no_side_effects.rs"]
mod dry_run_no_side_effects;
#[path = "dry_run_review_matrix.rs"]
mod dry_run_review_matrix;
#[path = "exec_exit_codes.rs"]
mod exec_exit_codes;
#[path = "exec_expect.rs"]
mod exec_expect;
#[path = "exec_for.rs"]
mod exec_for;
#[path = "exec_startup_budget.rs"]
mod exec_startup_budget;
#[path = "fail_closed_unreadable.rs"]
mod fail_closed_unreadable;
#[path = "fleet_denials_q8.rs"]
mod fleet_denials_q8;
#[path = "fleet_feed_q8.rs"]
mod fleet_feed_q8;
#[path = "fleet_preflight_q8.rs"]
mod fleet_preflight_q8;
#[path = "handshake_conformance.rs"]
mod handshake_conformance;
#[path = "issue171_matrix.rs"]
mod issue171_matrix;
#[path = "json_errors.rs"]
mod json_errors;
#[path = "json_failure_q8.rs"]
mod json_failure_q8;
#[path = "json_tty.rs"]
mod json_tty;
#[path = "mcp_supervised.rs"]
mod mcp_supervised;
#[path = "offboard_cmd.rs"]
mod offboard_cmd;
#[path = "onboard_marketplace.rs"]
mod onboard_marketplace;
#[path = "phantom_credential_contract.rs"]
mod phantom_credential_contract;
#[path = "pin_rewrite_configs.rs"]
mod pin_rewrite_configs;
#[path = "quickstart_then_doctor.rs"]
mod quickstart_then_doctor;
#[path = "quickstart_then_enter.rs"]
mod quickstart_then_enter;
#[path = "readme_commands.rs"]
mod readme_commands;
#[path = "readme_quickstart.rs"]
mod readme_quickstart;
#[path = "registry_manifest_ci_role.rs"]
mod registry_manifest_ci_role;
#[path = "registry_manifest_consumer_checklist.rs"]
mod registry_manifest_consumer_checklist;
#[path = "registry_manifest_current_state.rs"]
mod registry_manifest_current_state;
#[path = "registry_manifest_verification.rs"]
mod registry_manifest_verification;
#[path = "registry_signing_open_questions.rs"]
mod registry_signing_open_questions;
#[path = "registry_trust_faq.rs"]
mod registry_trust_faq;
#[path = "registry_unsigned_consumer_guidance.rs"]
mod registry_unsigned_consumer_guidance;
#[path = "regress_closed_issues.rs"]
mod regress_closed_issues;
#[path = "sample_exec_quiet.rs"]
mod sample_exec_quiet;
#[path = "setup_client_shapes.rs"]
mod setup_client_shapes;
#[path = "setup_gemini.rs"]
mod setup_gemini;
#[path = "setup_supervised_hint.rs"]
mod setup_supervised_hint;
#[path = "siem_export.rs"]
mod siem_export;
#[path = "stale_pin_recovery.rs"]
mod stale_pin_recovery;
#[path = "status_doctor_output.rs"]
mod status_doctor_output;
#[path = "status_pin_expiry.rs"]
mod status_pin_expiry;
#[path = "stdout_closed.rs"]
mod stdout_closed;
#[path = "verify_session_stream_contract.rs"]
mod verify_session_stream_contract;
#[path = "whoami_credential_present.rs"]
mod whoami_credential_present;

#[test]
fn all_cli_integration_files_are_registered() {
    let tests = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let harness = include_str!("integration.rs");
    let manifest: toml::Value = toml::from_str(include_str!("../Cargo.toml")).unwrap();
    let standalone_tests = manifest["test"].as_array().unwrap();
    for entry in std::fs::read_dir(tests).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|ext| ext == "rs")
            && path.file_name().unwrap() != "integration.rs"
        {
            let declaration = format!(
                "#[path = \"{}\"]",
                path.file_name().unwrap().to_str().unwrap()
            );
            let target_path = format!("tests/{}", path.file_name().unwrap().to_str().unwrap());
            let standalone = standalone_tests
                .iter()
                .any(|target| target["path"].as_str() == Some(target_path.as_str()));
            assert!(
                harness.contains(&declaration) || standalone,
                "missing {declaration} or explicit Cargo test target"
            );
        }
    }
}
