#!/usr/bin/env bash
# check-readme-quickstart.sh -- dry rehearsal of the README "30-second start".
#
# Extracts the locus commands from the README quickstart block and replays
# them verbatim inside a throwaway HOME (mktemp -d), in one shell, the way a
# first-run user would. Dry mode throughout: nothing touches the real
# ~/.locus, nothing is installed, no network or credentials are needed, and
# the temp home is removed on exit. A stale or broken quickstart fails here
# before it fails a new user.
#
# Documented expectations are checked, not just exit 0: a step whose README
# comment says "refused"/"fails" must exit nonzero, "exit N" pins the code,
# and the refusal must still say pin_mismatch with the child never started.
# The documented `--dry-run` surface is checked too: `quickstart --dry-run`
# and `enter --dry-run` must print a locus.plan.v1 plan and write nothing.
#
#   --bin PATH     locus binary to check (default: target/debug/locus, then
#                  target/release/locus, then `locus` on PATH)
#   --readme PATH  README to extract from (default: ./README.md at repo root)
#   --dry-run      print the extracted steps and expectations without running
#   -h, --help     this help
#
# Never prints the control capability, seals or any credential material.

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
README="$ROOT/README.md"
LOCUS_BIN=""
DRY=0

usage() { sed -n '2,23p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    --bin)     LOCUS_BIN="$2"; shift 2 ;;
    --readme)  README="$2";    shift 2 ;;
    --dry-run) DRY=1;          shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage; exit 2 ;;
  esac
done

if [[ -z "$LOCUS_BIN" ]]; then
  for c in "$ROOT/target/debug/locus" "$ROOT/target/release/locus"; do
    if [[ -x "$c" ]]; then LOCUS_BIN="$c"; break; fi
  done
fi
if [[ -z "$LOCUS_BIN" ]] && command -v locus >/dev/null 2>&1; then
  LOCUS_BIN="$(command -v locus)"
fi
if [[ -z "$LOCUS_BIN" || ! -x "$LOCUS_BIN" ]]; then
  echo "FAIL no locus binary; run cargo build -p locus-cli or pass --bin PATH" >&2
  exit 1
fi
LOCUS_BIN="$(cd "$(dirname "$LOCUS_BIN")" && pwd)/$(basename "$LOCUS_BIN")"
[[ -f "$README" ]] || { echo "FAIL README not found: $README" >&2; exit 1; }

pass=0
ok()   { printf '  ok  %s\n' "$*"; pass=$((pass + 1)); }
die()  { printf '  FAIL %s\n' "$*" >&2; exit 1; }

# Extract the fenced bash block under "## 30-second start": lines between the
# first ```bash fence after the heading and its closing ```. Keep only lines
# that invoke locus or eval the shell hook (install lines are skipped), with
# the trailing "# ..." comment kept for expectation parsing.
extract_steps() {
  awk '
    /^## 30-second start/ { in_sec = 1; next }
    in_sec && /^```bash/   { in_blk = 1; next }
    in_blk && /^```/       { exit }
    in_blk                 { print }
  ' "$README" | sed -E 's/^[[:space:]]+//; s/[[:space:]]+$//' \
    | grep -E '^(locus |eval "\$\(locus )'
}

# Expected exit code from a step's README comment: "exit N" is explicit;
# "refused"/"fails"/"denied" mean nonzero; anything else is 0.
expect_for() {
  local comment="$1" n
  if n="$(grep -oE 'exit [0-9]+' <<<"$comment" | grep -oE '[0-9]+' | head -1)" \
     && [[ -n "$n" ]]; then
    echo "$n"; return
  fi
  if grep -qiE 'refus|fail|denied' <<<"$comment"; then echo "nonzero"; return; fi
  echo "0"
}

STEPS=()
while IFS= read -r line; do STEPS+=("$line"); done < <(extract_steps)
if [[ ${#STEPS[@]} -lt 4 ]]; then
  die "extracted ${#STEPS[@]} locus step(s) from '## 30-second start' in $README; section missing or reshaped?"
fi
# The check shell is bash; the README's "or bash" alternative is what we run.
for i in "${!STEPS[@]}"; do STEPS[$i]="${STEPS[$i]//hook zsh/hook bash}"; done

if [[ $DRY -eq 1 ]]; then
  echo "dry run: would replay ${#STEPS[@]} extracted step(s) from $README"
  echo "  binary : $LOCUS_BIN"
  echo "  home   : fresh mktemp dir (removed on exit)"
  for s in "${STEPS[@]}"; do
    cmd="${s%%#*}"; cmd="${cmd%"${cmd##*[![:space:]]}"}"
    printf '  step   %-52s expect exit=%s\n' "$cmd" "$(expect_for "${s#*#}")"
  done
  cat <<'EOF'
  plus   quickstart --dry-run prints locus.plan.v1 and writes nothing
  plus   enter <other> --dry-run prints a plan and leaves the pin unchanged
  plus   refused exec --expect leaves no child side effects (marker check)
EOF
  exit 0
fi

echo "readme quickstart check"
echo "  binary : $LOCUS_BIN"
echo "  readme : $README (${#STEPS[@]} steps)"

# Throwaway first-run environment. HOME is redirected so the default
# ~/.locus resolution is exercised exactly as the README documents it; any
# ambient LOCUS_* state from the caller is scrubbed. The shell that runs the
# steps is this script, matching the README's single-shell flow, so the pin's
# supervised broker stays alive for whoami/exec.
WORK="$(mktemp -d "${TMPDIR:-/tmp}/locus-readme-qs.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
for v in $(env | sed -n 's/^\(LOCUS_[A-Za-z0-9_]*\)=.*/\1/p'); do unset "$v"; done
export HOME="$WORK/home"
mkdir -p "$HOME" "$WORK/ws"
export NO_COLOR=1 PATH="$(dirname "$LOCUS_BIN"):$PATH"
cd "$WORK/ws"

locus() { "$LOCUS_BIN" "$@"; }

step_out="$WORK/step.out"
run_step() {
  local line="$1"
  local cmd="${line%%#*}"; cmd="${cmd%"${cmd##*[![:space:]]}"}"
  local comment="" expect rc
  [[ "$line" == *'#'* ]] && comment="${line#*#}"
  expect="$(expect_for "$comment")"
  eval "$cmd" >"$step_out" 2>&1
  rc=$?
  case "$expect" in
    nonzero)
      [[ $rc -ne 0 ]] || die "\`$cmd\` unexpectedly succeeded (README documents a refusal)" ;;
    *)
      [[ $rc -eq "$expect" ]] || {
        sed 's/^/      | /' "$step_out" >&2
        die "\`$cmd\` exited $rc, README documents $expect"
      } ;;
  esac
  ok "$cmd"
}

for s in "${STEPS[@]}"; do run_step "$s"; done

# Claim checks: the steps above proved the commands run; these prove the
# specific promises the README text makes about them.
[[ -n "${LOCUS_CONTROL_CAPABILITY:-}" ]] \
  || die "hook eval did not export LOCUS_CONTROL_CAPABILITY (README: 'exports the control capability')"
ok "hook exported the control capability (value never printed)"

# whoami must show the pinned sample binding.
locus whoami >"$step_out" 2>&1 || die "locus whoami failed after enter"
grep -q "personal" "$step_out" \
  || die "whoami does not show the 'personal' pin: $(head -1 "$step_out")"
ok "whoami shows the personal pin"

# The documented refusal: exit 1, pin_mismatch, and the child never starts.
MARKER="$WORK/child-ran"
locus exec --expect acme -- sh -c 'touch "$1"' _ "$MARKER" >"$step_out" 2>&1
rc=$?
[[ $rc -eq 1 ]] || die "exec --expect acme exited $rc, documented 1"
grep -q "pin_mismatch" "$step_out" \
  || die "exec --expect refusal lost pin_mismatch: $(head -1 "$step_out")"
[[ ! -e "$MARKER" ]] || die "refused exec still started its child"
ok "exec --expect acme refused: exit 1, pin_mismatch, child never started"

# The documented --dry-run plan surface on the two mutating quickstart
# commands: a plan is printed and nothing is written.
SECOND_HOME="$WORK/second"
mkdir -p "$SECOND_HOME"
plan="$(HOME="$SECOND_HOME" "$LOCUS_BIN" --json --dry-run quickstart 2>&1)" \
  || die "quickstart --dry-run failed: $plan"
grep -q '"schema": *"locus.plan.v1"' <<<"$plan" \
  || die "quickstart --dry-run printed no locus.plan.v1 plan"
[[ ! -e "$SECOND_HOME/.locus/control_capability" ]] \
  || die "quickstart --dry-run wrote control_capability (must write nothing)"
[[ ! -d "$SECOND_HOME/.locus/bindings" ]] \
  || die "quickstart --dry-run wrote bindings (must write nothing)"
ok "quickstart --dry-run: locus.plan.v1 plan, nothing written"

plan="$(locus --json --dry-run enter acme 2>&1)" \
  || die "enter acme --dry-run failed: $plan"
grep -q '"schema": *"locus.plan.v1"' <<<"$plan" \
  || die "enter --dry-run printed no locus.plan.v1 plan"
grep -q '"acme"' <<<"$plan" || die "enter --dry-run plan does not name acme"
[[ "$(locus status --oneline 2>/dev/null)" == "personal:personal" ]] \
  || die "enter --dry-run changed the active pin"
ok "enter --dry-run: plans acme, active pin still personal"

printf '\nreadme quickstart PASS (%d checks)\n' "$pass"
