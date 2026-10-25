#!/usr/bin/env bash
# Fails unless the PR's workspace version is a plain MAJOR.MINOR.PATCH above
# both <base-ref>'s version and the highest vX.Y.Z tag, and Cargo.lock agrees.
# Usage: version-check.sh <base-ref> | version-check.sh --print-prerelease <version>
set -euo pipefail

if [ "${1:-}" = "--print-prerelease" ]; then
  [ "${2%%.*}" = "0" ] && echo true || echo false
  exit 0
fi
base="${1:?usage: version-check.sh <base-ref>}"

fail() { # fail <message> [<version to raise above>]
  echo "version check failed: $1" >&2
  [ -n "${2:-}" ] && echo "Raise \`[workspace.package] version\` in Cargo.toml above $2, then run \`cargo update -w\`." >&2
  exit 1
}
ws_version() { tr -d '\r' | awk '/^\[/{s=($0=="[workspace.package]")} s && /^version *=/{gsub(/^[^"]*"|".*$/,""); print; exit}'; }
# No pipe and no early exit: Cargo.lock outgrows the pipe buffer, so `tr | awk ...exit` could die of SIGPIPE under pipefail.
lock_version() { awk -v n="name = \"$1\"" '{sub(/\r$/,"")} !d && $0==n{getline; gsub(/^[^"]*"|".*$/,""); print; d=1}' Cargo.lock; }
plain() { [[ "$1" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]]; }
above() { [ "$1" != "$2" ] && [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | tail -n1)" = "$1" ]; }

pr=$(ws_version < Cargo.toml)
[ -n "$pr" ] || fail "no workspace version was found in Cargo.toml ([workspace.package] version)."
plain "$pr" || fail "version '$pr' must be MAJOR.MINOR.PATCH: digits only, no leading zeros, no prefix or suffix."

[ -f Cargo.lock ] || fail "Cargo.lock is missing."
for crate in lvpm lvpm-core; do
  locked=$(lock_version "$crate")
  [ "$locked" = "$pr" ] || fail "Cargo.lock records $crate ${locked:-<missing>} but Cargo.toml says $pr. Run \`cargo update -w\`."
done

tip=$(git show "$base:Cargo.toml" 2>/dev/null | ws_version) || fail "could not read $base:Cargo.toml."
plain "$tip" || fail "could not read a plain version from $base:Cargo.toml (got '$tip')."
mb=$(git merge-base HEAD "$base" 2>/dev/null) && forked=$(git show "$mb:Cargo.toml" | ws_version) || forked=""
if ! above "$pr" "$tip"; then
  [ -n "$forked" ] && [ "$forked" != "$tip" ] && fail "main is now $tip; rebase and raise above $tip." "$tip"
  fail "version $pr is not above $base's $tip." "$tip"
fi

tag=$(git tag -l 'v*' | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | sed 's/^v//' | sort -V | tail -n1 || true)
[ -z "$tag" ] || above "$pr" "$tag" || fail "version $pr is not above the highest release tag v$tag." "$tag"

echo "version check passed: $pr is above $base's $tip${tag:+ and tag v$tag}."
