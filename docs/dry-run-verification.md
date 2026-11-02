# Manual dry-run verification

[Issue #171](https://github.com/ashlrai/locus/issues/171) began with an audit
where most mutating commands did not offer a preview. The flag has since
expanded. This procedure verifies representative plans in the current checkout;
see the [command matrix](dry-run-matrix-q8.md) for the full classification.
It checks the planned actions **and** unchanged state, including the failed
precondition case. It does not apply any of the planned changes.

Build `locus` from this checkout (`cargo build -p locus-cli`), then run the
following as one Bash script from the repository root. It requires Python 3 and
uses a disposable home, Locus store and project. Set `LOCUS_BIN` to the absolute
path to another binary if needed. The script runs `init --with-samples` once to
create the fixture; its standard output is suppressed because initialization creates a
control capability. No real credentials or pin are needed.

```bash
set -eu
LOCUS_BIN=${LOCUS_BIN:-"$(pwd)/target/debug/locus"}
case "$LOCUS_BIN" in /*) ;; *) echo 'LOCUS_BIN must be absolute' >&2; exit 1;; esac
scratch=$(cd "$(mktemp -d)" && pwd -P)
trap 'rm -rf "$scratch"' EXIT
mkdir -p "$scratch/state/user" "$scratch/state/project"

# Keep LLVM runtime writes outside the observed tree, even after env -i.
# Resolve an inherited relative path before changing the CLI's working directory.
profile_file=$(python3 - "${LLVM_PROFILE_FILE:-$scratch/locus-%p-%m.profraw}" <<'PYPROFILE'
import os, sys
print(os.path.abspath(sys.argv[1]))
PYPROFILE
)

# Isolate the CLI from the operator's home, identity and workspace.
locus_test() {
  (cd "$scratch/state/project" && env -i PATH="$PATH" \
    HOME="$scratch/state/user" USERPROFILE="$scratch/state/user" \
    LOCUS_HOME="$scratch/state/locus" NO_COLOR=1 LOCUS_NON_INTERACTIVE=1 \
    LLVM_PROFILE_FILE="$profile_file" \
    "$LOCUS_BIN" "$@")
}

# Snapshot persistent paths, types, permissions, file mtimes, targets and hashes.
# Directory timestamps are bookkeeping; child paths detect directory mutations.
# Access/creation/change times are deliberately outside this comparison.
# The snapshot files and plan output are outside the tree being observed.
cat > "$scratch/snapshot.py" <<'PY'
import hashlib, json, os, stat, sys
from pathlib import Path
root = Path(sys.argv[1])
entries = {}
for base, dirs, files in os.walk(root, followlinks=False):
    for name in sorted(dirs + files):
        p = Path(base) / name
        s = p.lstat()
        entry = [stat.S_IFMT(s.st_mode), stat.S_IMODE(s.st_mode)]
        if not stat.S_ISDIR(s.st_mode):
            entry.append(s.st_mtime_ns)
        if stat.S_ISLNK(s.st_mode):
            entry.append(os.readlink(p))
        elif stat.S_ISREG(s.st_mode):
            entry.append(hashlib.sha256(p.read_bytes()).hexdigest())
        entries[str(p.relative_to(root))] = entry
print(json.dumps(entries, sort_keys=True))
PY
snapshot() { python3 "$scratch/snapshot.py" "$scratch/state" > "$1"; }
unchanged() {
  snapshot "$scratch/after.json"
  if ! cmp -s "$scratch/before.json" "$scratch/after.json"; then
    echo 'FAIL: paths, contents or metadata changed during preview' >&2
    python3 - "$scratch/before.json" "$scratch/after.json" <<'PYDIFF' >&2
import json, sys
before, after = (json.load(open(p, encoding='utf-8')) for p in sys.argv[1:])
for path in sorted(before.keys() | after.keys()):
    if before.get(path) != after.get(path):
        # Report fixture paths only, never file contents or credential material.
        print('Changed path:', path)
PYDIFF
    exit 1
  fi
}

# These command forms and their options must remain discoverable in current help.
for form in 'init' 'workspace' 'binding rm' 'pin'; do
  # Intentional word splitting: each form is a fixed command path above.
  locus_test $form --help > "$scratch/help"
  grep -q -- '--dry-run' "$scratch/help"
done

# Before initialization, init must describe creation without creating the store.
snapshot "$scratch/before.json"
locus_test init --with-samples --dry-run --json > "$scratch/plan.json"
unchanged
python3 - "$scratch/plan.json" "$scratch/state/locus" <<'PY'
import json, sys
from pathlib import Path
p = json.load(open(sys.argv[1], encoding="utf-8"))
home = Path(sys.argv[2])
assert p['schema'] == 'locus.plan.v1' and p['ok'] and p['dry_run']
changes = {(c['action'], c['target']) for c in p['changes']}
assert ('create', str(home)) in changes
assert ('create', str(home / 'bindings/personal.toml')) in changes
assert ('create', str(home / 'bindings/acme.toml')) in changes
assert not home.exists()
PY

# Only fixture setup writes state. Capture fresh baselines for each preview.
locus_test init --with-samples > /dev/null
for scenario in workspace remove pin missing; do
  snapshot "$scratch/before.json"
  case "$scenario" in
    workspace) locus_test workspace --default personal --dry-run --json > "$scratch/plan.json" ;;
    remove) locus_test binding rm personal --yes --dry-run --json > "$scratch/plan.json" ;;
    pin) locus_test pin personal --dry-run --json > "$scratch/plan.json" ;;
    missing)
      if locus_test pin absent --dry-run --json > "$scratch/plan.json" 2> "$scratch/error.json"; then
        echo 'FAIL: missing binding was accepted' >&2; exit 1
      else
        code=$?
        [ "$code" -eq 1 ] || { echo 'FAIL: unexpected exit code' >&2; exit 1; }
      fi ;;
  esac
  unchanged
  if [ "$scenario" = missing ]; then
    python3 - "$scratch/error.json" <<'PYERROR'
import json, sys
error = json.load(open(sys.argv[1], encoding="utf-8"))
assert 'absent' in error['error']
PYERROR
  else
    python3 - "$scratch/plan.json" "$scenario" "$scratch/state" <<'PY'
import json, sys
from pathlib import Path
p = json.load(open(sys.argv[1], encoding="utf-8"))
scenario, root = sys.argv[2], Path(sys.argv[3])
assert p['schema'] == 'locus.plan.v1' and p['ok'] and p['dry_run']
changes = {(c['action'], c['target']) for c in p['changes']}
if scenario == 'workspace':
    assert ('create', str(root / 'project/.locus.toml')) in changes
elif scenario == 'remove':
    assert ('delete', str(root / 'locus/bindings/personal.toml')) in changes
else:
    assert ('pin', 'personal') in changes
    assert ('create', str(root / 'locus/sessions/active.json')) in changes
PY
  fi
done
echo 'PASS: help, planned actions, failed precondition and unchanged state'
```

A nonzero exit, failed assertion, or changed snapshot is a failure; investigate
before trusting the preview. The snapshot compares paths, file bytes (by hash),
file types, modes and non-directory modification times. Directory timestamps
and access/change/creation times are excluded: they are not persistent file
contents or permissions. New or removed directories still change the path set;
coverage output has no blanket filename exemption inside state. The snapshot
cannot detect transient writes that are completely undone, directory timestamp
changes alone, or side effects outside the isolated tree. It also
does not establish that a later real operation is authorized or will succeed.
Do not run the fixture setup in a real Locus home. Read-only commands can accept
and ignore `--dry-run` and retain normal side effects; print-only setup forms and
unsupported child/server commands have different behavior. See the
[agent contract](agent-contract.md#--dry-run) for those limits.
