//! `locus doctor`: `locus.doctor.v1` JSON and the existing human output.

use super::{
    add_doctor_envelope, doctor_exit_code, gather_doctor_report, json_out, phantom_preflight,
    print_doctor_human, store,
};
use anyhow::Result;

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
