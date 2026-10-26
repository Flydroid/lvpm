#!/usr/bin/env bash
# Checks the e2e matrix and the PR workflows are wired safely: every row's image
# is an NI LabVIEW tag whose -linux/-windows suffix matches the row's os, no
# step condition names an image, the workflows a PR runs only read the repo,
# never run on a trigger that hands fork code secrets, and use no secret but
# GITHUB_TOKEN.
# Usage: e2e-matrix.sh [<workflow dir>]  (default .github/workflows)
set -euo pipefail
dir="${1:-.github/workflows}"
pr="$dir/pr.yml"
[ -f "$pr" ] || { echo "e2e lint: $pr not found" >&2; exit 1; }
errors=0
report_violation() { echo "e2e lint: $1" >&2; errors=$((errors+1)); }
# Read each file once: grep -q on a pipe can end tr with SIGPIPE, which pipefail reports.
pr_text=$(tr -d '\r' < "$pr")

rows=$(grep -E '^ *- \{.*\bimage:' <<<"$pr_text" || true)
[ -n "$rows" ] || report_violation "$pr: no e2e matrix rows with an image found."
while IFS= read -r row; do
  [ -n "$row" ] || continue
  os=$(sed -nE 's/.*\bos: *"?([a-z]+)"?.*/\1/p' <<<"$row")
  image=$(sed -nE 's/.*\bimage: *"?([^", }]+)"?.*/\1/p' <<<"$row")
  [[ "$image" =~ ^nationalinstruments/labview:[0-9]{4}q[0-9][a-z0-9]*-(windows|linux)$ ]] \
    || { report_violation "$pr: image '$image' is not a nationalinstruments/labview:<year>q<n>...-<windows|linux> tag."; continue; }
  [ "${BASH_REMATCH[1]}" = "$os" ] || report_violation "$pr: image '$image' does not match its row's os '$os'."
done <<<"$rows"

named=$(grep -nE '^ *if:.*(image|labview:|-linux|-windows)' <<<"$pr_text" || true)
[ -z "$named" ] || report_violation "$pr: a condition names an image; branch on the row's os instead: $named"

for f in pr.yml build.yml package.yml; do
  [ -f "$dir/$f" ] || { report_violation "$dir/$f not found."; continue; }
  text=$(tr -d '\r' < "$dir/$f")
  grep -qE '^permissions:$' <<<"$text" && grep -qE '^  contents: read$' <<<"$text" \
    || report_violation "$dir/$f must set top-level 'permissions: contents: read'."
  ! grep -qE '\b[a-z-]+: *["'\'']?write\b|permissions: *["'\'']?write-all' <<<"$text" \
    || report_violation "$dir/$f must not grant a write permission."
  ! grep -qE '\b(pull_request_target|workflow_run)\b' <<<"$text" \
    || report_violation "$dir/$f must not run on pull_request_target or workflow_run: both give fork code secrets."
  secrets=$(grep -noE 'secrets\.[A-Za-z0-9_]+|secrets\[|\(secrets\)|secrets: *inherit' <<<"$text" | grep -v 'secrets\.GITHUB_TOKEN$' || true)
  [ -z "$secrets" ] || report_violation "$dir/$f uses a secret other than GITHUB_TOKEN: $secrets"
done

[ $errors = 0 ] && echo "e2e lint passed: $dir" || exit 1
