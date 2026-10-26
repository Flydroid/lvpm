#!/usr/bin/env bash
# The CLI smoke flow inside a LabVIEW Linux image: list targets, search, check
# the lock, install, list, uninstall, and check no LabVIEW process ever ran.
# Usage: smoke.sh <lvpm binary> <fixture dir> <LabVIEW year the image has>
# LVPM_SMOKE_NATINST moves /usr/local/natinst, for the self-test only.
# smoke.ps1 is its Windows twin: keep the steps and wording the same.
set -euo pipefail
lvpm=$1 fixture=$2 year=$3
pkg=oglib_boolean version=6.0.0.9
installed=${LVPM_SMOKE_NATINST:-/usr/local/natinst}/LabVIEW-$year-64/user.lib/_OpenG.lib/boolean/boolean.llb
total=7 n=0 out=
summary=${GITHUB_STEP_SUMMARY:-/dev/null}
export LV_RTE_HEADLESS=1

work=$(mktemp -d)
seen=$work/.labview-seen
cp -r "$fixture/." "$work/project"
cd "$work/project"

# Sample the process table for LabVIEW while the flow runs. /proc needs
# nothing installed, unlike pgrep.
(
  shopt -s nullglob
  while :; do
    for c in /proc/[0-9]*/comm; do
      read -r name < "$c" 2>/dev/null || continue
      if [[ ${name,,} == labview ]]; then echo "$name" > "$seen"; fi
    done
    sleep 0.2
  done
) &
sampler=$!
trap 'kill $sampler 2>/dev/null || true; rm -rf "$work"' EXIT

fail() { # fail <message>: the last line of the log and of the job summary
  echo "::error::$1"; printf '%s\n\n```\n%s\n```\n' "$1" "$out" >> "$summary"; exit 1
}

# step <title> <command...>: runs the next step, keeps its output in $out.
step() {
  local title=$1; shift
  n=$((n+1))
  echo "::group::step $n/$total: $title: $*"
  local start=$SECONDS status=0
  out=$("$@" 2>&1) || status=$?
  echo "$out"
  echo "($((SECONDS-start))s)"
  echo "::endgroup::"
  if [ $status != 0 ]; then
    # reqwest's wording for a network failure: the registry, not lvpm.
    grep -qiE 'error sending request|dns error|connection refused|timed out|tcp connect' <<<"$out" \
      && fail "infrastructure: registry unreachable at step $n/$total: $*"
    fail "FAILED at step $n/$total: $* (exit $status)"
  fi
}

# expect <what> <condition...>: checks the step that just ran.
expect() {
  local what=$1; shift
  "$@" || fail "FAILED at step $n/$total: expected $what"
}

step "targets" "$lvpm" targets
expect "LabVIEW $year in the targets" grep -q "LabVIEW $year" <<<"$out"
step "search" "$lvpm" search "$pkg"
expect "$pkg in the search" grep -q "^$pkg " <<<"$out"
step "lock check" "$lvpm" lock --check
step "install" "$lvpm" install
expect "$installed to exist" test -e "$installed"
step "list" "$lvpm" list
expect "$pkg $version in the list" grep -qE "^$pkg +$version " <<<"$out"
step "uninstall" "$lvpm" uninstall "$pkg"
expect "$installed to be gone" test ! -e "$installed"
n=$((n+1)); out=
# The sampler never exits on its own: a dead one checked nothing.
kill -0 $sampler 2>/dev/null || fail "FAILED at step $n/$total: the LabVIEW sampler stopped"
kill $sampler 2>/dev/null || true; wait $sampler 2>/dev/null || true
[ ! -e "$seen" ] || fail "FAILED at step $n/$total: LabVIEW was started ($(cat "$seen"))"

echo "smoke flow passed all $total steps (LabVIEW $year)"
