//! Test-only helpers shared across `locus-core` unit tests.

/// Env var naming the single test a re-executed child process must run.
const ISOLATED_TEST_ENV: &str = "LOCUS_TEST_ISOLATED_CHILD";

/// Run the calling test in a fresh, single-threaded child process.
///
/// Process environment is global: a test that mutates security-relevant env
/// (`LOCUS_HOME`, `LOCUS_CONTROL_CAPABILITY`, trust keys, credential locators)
/// races every other test in the same libtest binary, which run in parallel
/// threads. Such tests call this first:
///
/// ```ignore
/// if !run_in_isolated_process(module_path!(), "my_test") { return; }
/// ```
///
/// In the parent it re-executes the current test binary filtered to exactly
/// this test (with ambient `LOCUS_*` stripped so the child is hermetic),
/// asserts the child ran exactly one test and passed, and returns `false`.
/// In the child it returns `true` and the body runs with the process to itself.
pub(crate) fn run_in_isolated_process(module_path: &str, test_fn: &str) -> bool {
    run_in_isolated_process_with_env(module_path, test_fn, &[])
}

/// [`run_in_isolated_process`], with `env` set in the child from its start
/// (never mutated in this shared test process).
pub(crate) fn run_in_isolated_process_with_env(
    module_path: &str,
    test_fn: &str,
    env: &[(&str, &str)],
) -> bool {
    // libtest names omit the crate segment of `module_path!()`.
    let name = match module_path.split_once("::") {
        Some((_, rest)) => format!("{rest}::{test_fn}"),
        None => test_fn.to_string(),
    };
    if std::env::var(ISOLATED_TEST_ENV).ok().as_deref() == Some(name.as_str()) {
        return true;
    }
    let exe = std::env::current_exe().expect("current test executable");
    let mut cmd = std::process::Command::new(exe);
    cmd.args([name.as_str(), "--exact", "--test-threads=1", "--nocapture"]);
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("LOCUS_") {
            cmd.env_remove(key);
        }
    }
    cmd.envs(env.iter().copied());
    cmd.env(ISOLATED_TEST_ENV, &name);
    let output = cmd.output().expect("spawn isolated test process");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "isolated test {name} failed ({}):\n--- stdout\n{stdout}\n--- stderr\n{stderr}",
        output.status
    );
    // Guard against a filter typo silently running zero tests.
    assert!(
        stdout.contains("test result: ok. 1 passed"),
        "isolated test {name} did not run exactly one test:\n{stdout}"
    );
    false
}
