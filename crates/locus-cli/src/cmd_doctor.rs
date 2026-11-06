//! `locus doctor`: `locus.doctor.v1` JSON and the existing human output.

use super::{
    add_doctor_envelope, exit, gather_doctor_report, json_out, phantom_preflight,
    print_doctor_human, store,
};
use anyhow::Result;
use locus_core::DoctorVerdict;

pub(super) fn run(json: bool) -> Result<()> {
    let s = store()?;

    // Continuous whoami: freeze session if binding material drifted under pin.
    // Best-effort — a wedged session must not abort doctor itself;
    // build_doctor_report re-runs this and classifies the wedge as a
    // stale_session finding with the `locus leave --force` recovery.
    let _ = s.check_drift_and_freeze();

    let report = gather_doctor_report(&s)?;

    // Opt-in (LOCUS_PHANTOM_PREFLIGHT=1 or strict): Phantom's value-free agent
    // preflight. Verdict and check ids only; a refusal lifts SAFE to exit 1.
    let preflight = phantom_preflight::evaluate(
        phantom_preflight::mode_from(std::env::var("LOCUS_PHANTOM_PREFLIGHT").ok().as_deref()),
        phantom_preflight::run_phantom,
    );

    if json {
        let mut value = serde_json::to_value(&report)?;
        if let (Some(p), Some(obj)) = (&preflight, value.as_object_mut()) {
            obj.insert("phantom_preflight".into(), serde_json::to_value(p)?);
        }
        let refused = preflight.as_ref().is_some_and(|p| p.refuse);
        add_doctor_envelope(&mut value, refused);
        json_out::print(json_out::DOCTOR, &value)?;
    } else {
        print_doctor_human(&report);
        if let Some(p) = &preflight {
            match &p.message {
                Some(m) => println!("{m}"),
                None => println!("phantom preflight: ready"),
            }
        }
    }

    let refused = preflight.as_ref().is_some_and(|p| p.refuse);
    doctor_exit_code(&report, refused).exit_unless_success();
    Ok(())
}

/// First-run setup advice stays visible without failing a diagnostic command.
/// Keep an explicit allowlist: new or unknown warnings remain failures, and
/// neither this exit status nor an advisory finding grants provider access.
fn doctor_exit_code(report: &locus_core::DoctorReport, refused: bool) -> exit::ExitCode {
    if report.verdict == DoctorVerdict::Unsafe {
        return exit::ExitCode::Unsafe;
    }
    if refused {
        return exit::ExitCode::Failure;
    }
    if report.verdict == DoctorVerdict::Warn
        && !report.findings.is_empty()
        && report.findings.iter().all(|finding| {
            finding.severity == locus_core::IssueSeverity::Info
                || (finding.severity == locus_core::IssueSeverity::Warn
                    && matches!(
                        finding.code.as_str(),
                        "approvals_missing"
                            | "phantom_missing"
                            | "control_capability_not_exported"
                            | "executor_authority_unavailable"
                    ))
        })
    {
        return exit::ExitCode::Success;
    }
    exit::ExitCode::from_doctor(report.verdict)
}

#[cfg(test)]
mod doctor_exit_tests {
    use super::{doctor_exit_code, exit};
    use locus_core::{build_doctor_report, DoctorExternal, DoctorVerdict, Store};
    use tempfile::tempdir;

    #[test]
    fn doctor_advisories_do_not_hide_preflight_refusals_or_unknown_findings() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path().join("locus-home")).unwrap();
        let mut report = build_doctor_report(
            &store,
            DoctorExternal {
                cwd: Some(dir.path().to_path_buf()),
                ..Default::default()
            },
        )
        .unwrap();
        report.findings.clear();
        report.issues.clear();
        report.verdict = DoctorVerdict::Safe;
        report.push_finding(
            locus_core::IssueSeverity::Warn,
            "phantom_missing",
            "optional installation".into(),
        );
        assert_eq!(doctor_exit_code(&report, false), exit::ExitCode::Success);
        assert_eq!(doctor_exit_code(&report, true), exit::ExitCode::Failure);
        report.push_finding(
            locus_core::IssueSeverity::Warn,
            "future_unknown_warning",
            "requires operator attention".into(),
        );
        assert_eq!(doctor_exit_code(&report, false), exit::ExitCode::Failure);
        report.push_finding(
            locus_core::IssueSeverity::Unsafe,
            "invalid_seal",
            "invalid seal".into(),
        );
        assert_eq!(doctor_exit_code(&report, false), exit::ExitCode::Unsafe);
        assert_eq!(doctor_exit_code(&report, true), exit::ExitCode::Unsafe);
    }

    #[test]
    fn each_advisory_is_allowed_only_at_warning_severity() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path().join("locus-home")).unwrap();
        let mut report = build_doctor_report(
            &store,
            DoctorExternal {
                cwd: Some(dir.path().to_path_buf()),
                ..Default::default()
            },
        )
        .unwrap();
        for code in [
            "approvals_missing",
            "phantom_missing",
            "control_capability_not_exported",
            "executor_authority_unavailable",
        ] {
            report.findings.clear();
            report.issues.clear();
            report.verdict = DoctorVerdict::Warn;
            report.push_finding(locus_core::IssueSeverity::Warn, code, String::new());
            assert_eq!(
                doctor_exit_code(&report, false),
                exit::ExitCode::Success,
                "{code}"
            );
            report.push_finding(locus_core::IssueSeverity::Unsafe, code, String::new());
            assert_eq!(
                doctor_exit_code(&report, false),
                exit::ExitCode::Unsafe,
                "{code}"
            );
        }
        report.findings.clear();
        report.verdict = DoctorVerdict::Warn;
        assert_eq!(doctor_exit_code(&report, false), exit::ExitCode::Failure);
    }
}
