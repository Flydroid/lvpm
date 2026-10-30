#!/usr/bin/env bash
# Fails unless the PR title is a conventional commit release-please can read
# (PRs are squash-merged, so the title becomes the commit on main).
# Usage: pr-title.sh <title>
set -euo pipefail
re='^(feat|fix|perf|refactor|docs|test|build|ci|chore|revert|style)(\([^)]+\))?!?: .+'
[[ "${1:-}" =~ $re ]] && { echo "ok: $1"; exit 0; }
echo "::error::PR title '${1:-}' is not a conventional commit, e.g. 'feat(sources): ...' or 'fix: ...'."
exit 1
