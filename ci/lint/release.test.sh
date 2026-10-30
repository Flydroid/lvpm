#!/usr/bin/env bash
# Self-test for release.sh: the real workflow passes, and each broken copy fails.
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
real="$here/../../.github/workflows/release.yml"
work=$(mktemp -d); trap 'rm -rf "$work"' EXIT
failures=0

expect() { # expect <0|1> <name> <sed script or empty> [<substring>]
  local want=$1 name=$2 edit=$3 msg=${4:-}
  sed -e "$edit" "$real" > "$work/release.yml"
  local out; out=$(bash "$here/release.sh" "$work/release.yml" 2>&1); local got=$?
  if [ "$got" = "$want" ] && [[ "$out" == *"$msg"* ]]; then echo "ok   $name"
  else echo "FAIL $name (exit $got): $out"; failures=$((failures+1)); fi
}

expect 0 "the real workflow passes" '' "release lint passed"
expect 1 "build without guard fails" '/^  build:/,/^  [a-z]/s/needs: \[guard\]/needs: []/' "job 'build' must need guard"
expect 1 "build without the release gate fails" "/^    if: needs.guard.outputs.release == 'true'/d" "job 'build' must have"
expect 1 "publish without guard fails" 's/needs: \[guard, package\]/needs: [package]/' "job 'publish' must need guard"
expect 1 "create without --draft fails" 's/gh release create "$TAG" --draft/gh release create "$TAG"/' "must pass --draft"
expect 1 "an upload after publishing fails" '$a\      - run: gh release upload "$TAG" extra' "last gh release call"
expect 1 "a git tag step fails" '$a\      - run: git tag v1.0.0' "no git tag"
expect 1 "a cargo test step fails" '$a\      - run: cargo test' "cargo test"
expect 1 "a missing report job fails" 's/^  release-report:/  report:/' "release-report"
expect 1 "a report only on failure fails" "s/^    if: always() && (contains.*/    if: failure()/" "failed or was cancelled"
expect 1 "publish on any branch fails" "s/github.ref == 'refs\/heads\/main' && //" "only on refs/heads/main"
expect 1 "publish on a dry run fails" "s/ && !inputs.dry_run//" "not on a dry run"
expect 1 "publish on main or outside a dry run fails" "s/^    if: github.ref == 'refs\/heads\/main' &&/    if: github.ref == 'refs\/heads\/main' ||/" "only on refs/heads/main"
expect 1 "a dry run in the release group fails" "s/^  group: .*/  group: release/" "concurrency group"
expect 1 "a cancelling publish job fails" '/^  publish:/a\    concurrency: { group: x, cancel-in-progress: true }' "cancel-in-progress: true"
expect 1 "a cancelling concurrency group fails" 's/cancel-in-progress: false/cancel-in-progress: true/' "cancel-in-progress"
expect 1 "publish without the re-check fails" '/Check the version is still not released/,+1d' "before 'gh release create'"
expect 1 "a re-check after create fails" '/Check the version is still not released/,+1d; /Publish the release/i\      - run: bash ci/version-check.sh --release' "before 'gh release create'"
expect 1 "a git push --tags step fails" '$a\      - run: git push origin --tags' "no git tag"

[ $failures = 0 ] && echo "all release lint scenarios passed" || { echo "$failures scenario(s) failed"; exit 1; }
