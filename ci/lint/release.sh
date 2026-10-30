#!/usr/bin/env bash
# Checks the release workflow is wired safely: build and publish need guard,
# build runs only when guard decides to release,
# publish runs only on main outside a dry run and re-checks the version first,
# a running release is never cancelled, the release is created as a draft and
# made public only as the last gh call, nothing else tags, no tests run, and
# the failure report job runs on failure and cancellation.
# Usage: release.sh [<workflow file>]  (default .github/workflows/release.yml)
set -euo pipefail
file="${1:-.github/workflows/release.yml}"
[ -f "$file" ] || { echo "release lint: $file not found" >&2; exit 1; }
errors=0
report_violation() { echo "release lint: $file: $1" >&2; errors=$((errors+1)); }

# Prints the value of <key> in job <job> (a two-space-indented key under jobs:).
job_key() { tr -d '\r' < "$file" | awk -v job="$1" -v key="$2" '
  /^jobs:/ {j=1; next}  /^[^ ]/ {j=0}
  j && /^  [A-Za-z0-9_-]+:/ {cur=$1; sub(":$","",cur); next}
  j && cur==job && $0 ~ "^    "key":" {sub("^    "key": *",""); print; exit}'; }

for job in build publish; do
  [[ "$(job_key "$job" needs)" =~ (^|[^a-z-])guard([^a-z-]|$) ]] || report_violation "job '$job' must need guard."
done
[ "$(job_key build if)" = "needs.guard.outputs.release == 'true'" ]   || report_violation "job 'build' must have 'if: needs.guard.outputs.release == '''true'''' (only when guard decides to release)."
report_if=$(job_key release-report if)
[[ "$report_if" == *"'failure'"* && "$report_if" == *"'cancelled'"* ]] \
  || report_violation "job 'release-report' must run when a job failed or was cancelled."
publish_gate="github.ref == 'refs/heads/main' && !inputs.dry_run"
[ "$(job_key publish if)" = "$publish_gate" ] \
  || report_violation "job 'publish' must have 'if: $publish_gate' (only on refs/heads/main and not on a dry run)."
# No pipe into grep -q: its early exit would fail the pipe under pipefail. \s also matches a CR.
grep -qE '^  cancel-in-progress: false\s*$' "$file" || report_violation "concurrency must set 'cancel-in-progress: false'."
! grep -qE 'cancel-in-progress: *true' "$file" || report_violation "no 'cancel-in-progress: true' allowed."
grep -qF "  group: \${{ $publish_gate && 'release' || 'release-no-publish' }}" "$file" \
  || report_violation "the concurrency group must put only runs that can publish in group 'release'."
publish_body=$(tr -d '\r' < "$file" | awk '/^  publish:/ {p=1; next} /^  [A-Za-z0-9_-]+:/ {p=0} p')
check_line=$(grep -n 'version-check.sh --release' <<<"$publish_body" | head -n1 | cut -d: -f1 || true)
create_line=$(grep -n 'gh release create' <<<"$publish_body" | head -n1 | cut -d: -f1 || true)
[ -n "$check_line" ] && { [ -z "$create_line" ] || [ "$check_line" -lt "$create_line" ]; } \
  || report_violation "job 'publish' must run 'ci/version-check.sh --release' before 'gh release create'."

gh_lines=$(tr -d '\r' < "$file" | grep -E '^[^#]*\bgh release (create|upload|edit|delete)\b' || true)
first_create=$(grep -m1 'gh release create' <<<"$gh_lines" || true)
[ -n "$first_create" ] || report_violation "no 'gh release create' found."
[[ "$first_create" == *--draft* ]] || report_violation "the first 'gh release create' must pass --draft."
last=$(tail -n1 <<<"$gh_lines")
[[ "$last" == *"gh release edit"*"--draft=false"* ]] || report_violation "the last gh release call must be 'gh release edit ... --draft=false'."

forbidden=$(tr -d '\r' < "$file" | grep -nE '^[^#]*(\bgit tag\b|\bgit push\b.*--tags|\bcargo test\b)' || true)
[ -z "$forbidden" ] || report_violation "no git tag, git push --tags or cargo test allowed: $forbidden"

[ $errors = 0 ] && echo "release lint passed: $file" || exit 1
