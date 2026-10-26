#!/usr/bin/env bash
# Self-test for smoke.sh, with a stub lvpm: the flow passes, a failing step
# names itself, a LabVIEW process fails the run (the sampler's positive
# control), and a network failure is reported as infrastructure.
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
work=$(mktemp -d); trap 'rm -rf "$work"' EXIT
failures=0
export LVPM_SMOKE_NATINST="$work/natinst"
llb="$LVPM_SMOKE_NATINST/LabVIEW-2026-64/user.lib/_OpenG.lib/boolean/boolean.llb"

# A stub lvpm. STUB_FAIL names the subcommand that fails; STUB_LABVIEW=1
# starts a process named labview during install; STUB_OFFLINE=1 makes search
# fail the way reqwest does without a network.
cat > "$work/lvpm" <<EOF
#!/usr/bin/env bash
[ "\$1" = "\${STUB_FAIL:-}" ] && { echo "stub: \$1 failed"; exit 3; }
case \$1 in
  targets) echo "LabVIEW 2026 (64-bit)  v26  /usr/local/natinst/LabVIEW-2026-64" ;;
  search)  [ -n "\${STUB_OFFLINE:-}" ] && { echo "Error: fetching http://x: error sending request"; exit 1; }
           echo "oglib_boolean   6.0.0.9   OpenG Boolean Library" ;;
  lock)    echo "lvpm.lock is up to date" ;;
  install) mkdir -p "$llb"
           [ -n "\${STUB_LABVIEW:-}" ] && { cp "\$(command -v sleep)" "$work/labview"; "$work/labview" 3 & sleep 1; }
           echo "done" ;;
  list)    echo "oglib_boolean   6.0.0.9   5 files" ;;
  uninstall) rm -rf "$llb"; echo "removed" ;;
esac
EOF
chmod +x "$work/lvpm"

expect() { # expect <0|1> <name> <last-line substring> [ENV=value...]
  local want=$1 name=$2 msg=$3; shift 3
  rm -rf "$LVPM_SMOKE_NATINST"; : > "$work/summary"
  local out got
  out=$(env "$@" GITHUB_STEP_SUMMARY="$work/summary" bash "$here/smoke.sh" "$work/lvpm" "$here/fixture" 2026 2>&1); got=$?
  local last; last=$(tail -n1 <<<"$out")
  if [ "$got" = "$want" ] && [[ "$last" == *"$msg"* ]] && { [ "$want" = 0 ] || grep -qF "$msg" "$work/summary"; }; then
    echo "ok   $name"
  else
    echo "FAIL $name (exit $got): $out"; failures=$((failures+1))
  fi
}

expect 0 "the flow passes" "smoke flow passed all 7 steps (LabVIEW 2026)"
expect 1 "a failing lock check stops at step 3" "FAILED at step 3/7: $work/lvpm lock --check (exit 3)" STUB_FAIL=lock
expect 1 "a LabVIEW process fails the flow" "FAILED at step 7/7: LabVIEW was started (labview)" STUB_LABVIEW=1
expect 1 "an unreachable registry is infrastructure" "infrastructure: registry unreachable at step 2/7" STUB_OFFLINE=1
expect 1 "a missing package file fails install" "FAILED at step 4/7: expected /nonexistent/natinst/LabVIEW-2026-64/user.lib/_OpenG.lib/boolean/boolean.llb to exist" LVPM_SMOKE_NATINST=/nonexistent/natinst

[ $failures = 0 ] && echo "all smoke scenarios passed" || { echo "$failures scenario(s) failed"; exit 1; }
