#!/usr/bin/env bash
# Self-test for pr-title.sh: each title is accepted (0) or rejected (1).
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
failures=0
expect() { # expect <0|1> <title>
  bash "$here/pr-title.sh" "$2" >/dev/null 2>&1; local got=$?
  if [ "$got" = "$1" ]; then echo "ok   $1 '$2'"; else echo "FAIL want $1 got $got: '$2'"; failures=$((failures+1)); fi
}
expect 0 "feat: add search"
expect 0 "fix(sources): the first source wins"
expect 0 "feat!: drop lvpm.lock v1"
expect 0 "refactor(core)!: rename OpEnv"
expect 0 "chore(main): release 0.14.0"
expect 0 "ci: release-please raises the version instead of every PR"
expect 1 "Add search"
expect 1 "feat add search"
expect 1 "feat:add search"
expect 1 "feature: add search"
expect 1 "feat(): add search"
expect 1 "feat: "
expect 1 ""
[ $failures = 0 ] && echo "all pr-title scenarios passed" || { echo "$failures scenario(s) failed"; exit 1; }
