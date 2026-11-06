//! Exercise the snapshot users copy, rather than a parallel implementation.
use std::process::Command;

#[test]
fn snapshot_ignores_reads_and_external_output_but_detects_state_writes() {
    let doc = include_str!("../../../docs/dry-run-verification.md");
    let snapshot = doc
        .split("<<'PY'\n")
        .nth(1)
        .unwrap()
        .split("\nPY\n")
        .next()
        .unwrap();
    let root = tempfile::tempdir().unwrap();
    let script = root.path().join("snapshot.py");
    std::fs::write(&script, snapshot).unwrap();
    let output = Command::new("python3")
        .args([
            "-c",
            r#"
import os, subprocess, sys
from pathlib import Path
script, root = sys.argv[1], Path(sys.argv[2])
state = root / 'state'
state.mkdir()
fixture = state / 'binding.toml'
fixture.write_text('synthetic binding')
fixture.chmod(0o644)
link = state / 'link'
link.symlink_to('binding.toml')
directory = state / 'existing-directory'
directory.mkdir()
directory.chmod(0o755)

def snapshot():
    return subprocess.check_output([sys.executable, script, str(state)])

def changed(action):
    before = snapshot()
    action()
    assert snapshot() != before, 'missed mutation: ' + action.__name__

before = snapshot()
# Explicit atime change avoids filesystem mount policies hiding this case.
s = fixture.stat()
os.utime(fixture, ns=(s.st_atime_ns + 1000000000, s.st_mtime_ns))
s = directory.stat()
os.utime(directory, ns=(s.st_atime_ns + 1000000000, s.st_mtime_ns + 1000000000))
fixture.read_bytes()
list(state.iterdir())
(root / 'coverage.profraw').write_bytes(b'synthetic profile')
(root / 'plan.json').write_text('{}')
assert snapshot() == before, 'reads or external output changed snapshot'

changed(lambda: directory.chmod(0o700))
changed(lambda: directory.rmdir())
# A write must be caught even when its timestamp is preserved.
def rewrite():
    s = fixture.stat()
    fixture.write_text('X' * s.st_size)
    os.utime(fixture, ns=(s.st_atime_ns, s.st_mtime_ns))
changed(rewrite)
changed(lambda: fixture.chmod(0o600))
s = fixture.stat()
changed(lambda: os.utime(fixture, ns=(s.st_atime_ns, s.st_mtime_ns + 1000000000)))
changed(lambda: (state / 'unexpected.profraw').write_bytes(b'not exempt in state'))
# Broker authority is still state, never a blanket runtime-directory exemption.
runtime = state / 'locus' / 'runtime'
runtime.mkdir(parents=True)
authority = runtime / 'authority.json'
changed(lambda: authority.write_text('synthetic authority'))
changed(lambda: authority.write_text('changed authority'))
changed(lambda: authority.rename(runtime / 'authority-retired.json'))
changed(lambda: (state / 'empty-directory').mkdir())
changed(lambda: fixture.rename(state / 'renamed.toml'))
def retarget():
    link.unlink()
    link.symlink_to('renamed.toml')
changed(retarget)
changed(lambda: link.unlink())
changed(lambda: (state / 'renamed.toml').unlink())
# Replacing a file by a directory must also be visible.
replacement = state / 'replacement'
replacement.write_text('')
def replace_type():
    replacement.unlink()
    replacement.mkdir()
changed(replace_type)
"#,
        ])
        .arg(&script)
        .arg(root.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
