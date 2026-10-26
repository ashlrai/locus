//! Exercise JSON commands with a real terminal on stdin, held open without input.
//! Python's standard-library PTY support avoids adding a platform dependency.
#![cfg(unix)]

use std::process::Command;

#[test]
fn json_commands_never_prompt_on_a_tty() {
    let home = tempfile::tempdir().unwrap();
    let output = Command::new("python3")
        .arg("-c")
        .arg(
            r#"
import json, os, pty, subprocess, sys
binary, home = sys.argv[1:]
env = os.environ.copy()
for key in ('LOCUS_SESSION_ID', 'LOCUS_CONTROL_CAPABILITY', 'LOCUS_NON_INTERACTIVE',
            'LOCUS_AUTHORITY_BROKER_ENDPOINT', 'LOCUS_AUTHORITY_BROKER_TOKEN'):
    env.pop(key, None)
env['LOCUS_HOME'] = home
env['HOME'] = home
env['NO_COLOR'] = '1'
# Synthetic test capability only; never adopt a real user's stored capability.
env['LOCUS_CONTROL_CAPABILITY'] = '01' * 32
master, slave = pty.openpty()
assert os.isatty(slave)
def run(args, code):
    with subprocess.Popen([binary] + args, stdin=slave, stdout=subprocess.PIPE,
                          stderr=subprocess.PIPE, env=env, cwd=home) as child:
        try:
            out, err = child.communicate(timeout=10)
        except subprocess.TimeoutExpired:
            child.kill()
            child.communicate()
            raise AssertionError('blocked waiting for terminal input: ' + repr(args))
    assert child.returncode == code, (args, child.returncode, out, err)
    assert b'[y/N]' not in out + err and b'[Y/n]' not in out + err, (args, out, err)
    if code:
        assert not out, (args, out)
        report = json.loads(err)
        assert report['ok'] is False, report
    else:
        assert not err, (args, err)
        report = json.loads(out)
    return report
try:
    run(['binding', 'add', 'fixture', '--tenant', 'fixture', '--provider', 'github',
         '--account', 'fixture', '--credential-ref', 'env:UNSET_FIXTURE', '--json'], 0)
    binding = os.path.join(home, 'bindings', 'fixture.toml')
    original = open(binding, 'rb').read()
    for prefix, suffix in [(['--json'], []), ([], ['--json'])]:
        def check(args, code):
            return run(prefix + args + suffix, code)
        report = check(['adapter', 'uninstall', 'fixture'], 2)
        assert '--yes' in report['error'], report
        report = check(['binding', 'rm', 'fixture'], 1)
        assert '--yes' in report['error'], report
        assert open(binding, 'rb').read() == original
        assert check(['leave'], 0)['left'] is False
        assert check(['leave', '--force'], 0)['left'] is False
        check(['binding', 'add'], 1)
        check(['client', 'add'], 1)
        check(['approve', 'grant', 'fixture', '--touchid'], 2)
        assert check(['adapter', 'uninstall', 'fixture', '--yes'], 0)['removed'] is False
    assert run(['binding', 'rm', 'fixture', '--yes', '--json'], 0)['removed'] == 'fixture'
    assert not os.path.exists(binding)
finally:
    os.close(slave)
    os.close(master)
"#,
        )
        .arg(env!("CARGO_BIN_EXE_locus"))
        .arg(home.path())
        .output()
        .expect("python3 with standard-library PTY support is required");
    assert!(
        output.status.success(),
        "PTY regression failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
