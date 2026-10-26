#!/usr/bin/env bash
# Self-test for e2e-matrix.sh: the real workflows pass, and each broken copy fails.
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
real="$here/../../.github/workflows"
work=$(mktemp -d); trap 'rm -rf "$work"' EXIT
failures=0

expect() { # expect <0|1> <name> <file to break> <sed script or empty> [<substring>]
  local want=$1 name=$2 file=$3 edit=$4 msg=${5:-}
  rm -rf "$work/wf"; cp -r "$real" "$work/wf"
  sed -i -e "$edit" "$work/wf/$file"
  # A sed that matches nothing would test the real files under a broken name.
  if [ -n "$edit" ] && cmp -s "$real/$file" "$work/wf/$file"; then
    echo "FAIL $name: the edit changed nothing"; failures=$((failures+1)); return
  fi
  local out; out=$(bash "$here/e2e-matrix.sh" "$work/wf" 2>&1); local got=$?
  if [ "$got" = "$want" ] && [[ "$out" == *"$msg"* ]]; then echo "ok   $name"
  else echo "FAIL $name (exit $got): $out"; failures=$((failures+1)); fi
}

expect 0 "the real workflows pass" pr.yml '' "e2e lint passed"
expect 1 "an image from another registry fails" pr.yml 's#nationalinstruments/labview:2026q3-linux#someone/labview:2026q3-linux#' "is not a nationalinstruments/labview"
expect 1 "a linux image on a windows row fails" pr.yml 's#2026q3-windows#2026q3-linux#' "does not match its row's os 'windows'"
expect 1 "an image without an os suffix fails" pr.yml 's#2025q3patch1-linux#2025q3patch1#' "is not a nationalinstruments/labview"
expect 1 "a condition naming an image fails" pr.yml "s/if: matrix.os == 'windows'/if: contains(matrix.image, '-windows')/" "names an image"
expect 1 "no matrix rows fails" pr.yml '/image: "nationalinstruments/d' "no e2e matrix rows"
expect 1 "write permission fails" build.yml 's/^  contents: read/  contents: write/' "build.yml must set"
expect 1 "a quoted write permission fails" build.yml 's/^  contents: read/  contents: read\n  packages: "write"/' "must not grant a write"
expect 1 "a missing permissions block fails" package.yml '/^permissions:/d' "package.yml must set"
expect 1 "a job-level write grant fails" pr.yml '/^  e2e:/a\    permissions: { packages: write }' "must not grant a write permission"
expect 1 "a latest tag fails" pr.yml 's#labview:2026q3-linux#labview:latest#' "is not a nationalinstruments/labview"
expect 1 "pull_request_target fails" pr.yml 's/^  pull_request:/  pull_request_target:/' "must not run on pull_request_target"
expect 1 "inherited secrets fail" pr.yml '/^  package:/a\    secrets: inherit' "secrets: inherit"
expect 1 "a secret fails" pr.yml '$a\        env: { T: ${{ secrets.NI_TOKEN }} }' "secrets.NI_TOKEN"

[ $failures = 0 ] && echo "all e2e lint scenarios passed" || { echo "$failures scenario(s) failed"; exit 1; }
