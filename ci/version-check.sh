#!/usr/bin/env bash
# Fails unless the PR's workspace version is a plain MAJOR.MINOR.PATCH above
# both <base-ref>'s version and the highest vX.Y.Z tag, and Cargo.lock agrees.
# --release (on main, before a release) skips the base comparison and also
# fails if tag v<version> already exists.
# Usage: version-check.sh <base-ref> | --release | --print-version | --print-prerelease <version>
set -euo pipefail

usage="usage: version-check.sh <base-ref> | --release | --print-version | --print-prerelease <version>"
release=
case "${1:-}" in
  --print-prerelease) [ "${2%%.*}" = "0" ] && echo true || echo false; exit 0 ;;
  --print-version) ;;
  --release) release=1 ;;
  *) base="${1:?$usage}" ;;
esac

fail() { # fail <message> [<version to raise above>]
  local lines=("version check failed: $1")
  if [ -n "${2:-}" ] && [ -n "$release" ]; then
    lines+=("Nothing was tagged or published. Raise the version above $2 in a new PR.")
  elif [ -n "${2:-}" ]; then
    lines+=("Raise \`[workspace.package] version\` in Cargo.toml above $2, then run \`cargo update -w\`.")
  fi
  printf '%s\n' "${lines[@]}" >&2
  [ -z "${GITHUB_STEP_SUMMARY:-}" ] || printf '%s\n\n' "${lines[@]}" >> "$GITHUB_STEP_SUMMARY"
  exit 1
}
ws_version() { tr -d '\r' | awk '/^\[/{s=($0=="[workspace.package]")} s && /^version *=/{gsub(/^[^"]*"|".*$/,""); print; exit}'; }
# No pipe and no early exit: Cargo.lock outgrows the pipe buffer, so `tr | awk ...exit` could die of SIGPIPE under pipefail.
lock_version() { awk -v n="name = \"$1\"" '{sub(/\r$/,"")} !d && $0==n{getline; gsub(/^[^"]*"|".*$/,""); print; d=1}' Cargo.lock; }
plain() { [[ "$1" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]]; }
above() { [ "$1" != "$2" ] && [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | tail -n1)" = "$1" ]; }

version=$(ws_version < Cargo.toml)
[ -n "$version" ] || fail "no workspace version was found in Cargo.toml ([workspace.package] version)."
plain "$version" || fail "version '$version' must be MAJOR.MINOR.PATCH: digits only, no leading zeros, no prefix or suffix."
[ "${1:-}" = "--print-version" ] && { echo "$version"; exit 0; }

[ -f Cargo.lock ] || fail "Cargo.lock is missing."
for crate in lvpm lvpm-core; do
  locked=$(lock_version "$crate")
  [ "$locked" = "$version" ] || fail "Cargo.lock records $crate ${locked:-<missing>} but Cargo.toml says $version. Run \`cargo update -w\`."
done

if [ -z "$release" ]; then
  tip=$(git show "$base:Cargo.toml" 2>/dev/null | ws_version) || fail "could not read $base:Cargo.toml."
  plain "$tip" || fail "could not read a plain version from $base:Cargo.toml (got '$tip')."
  mb=$(git merge-base HEAD "$base" 2>/dev/null) && forked=$(git show "$mb:Cargo.toml" | ws_version) || forked=""
  if ! above "$version" "$tip"; then
    [ -n "$forked" ] && [ "$forked" != "$tip" ] && fail "main is now $tip; rebase and raise above $tip." "$tip"
    fail "version $version is not above $base's $tip." "$tip"
  fi
fi

if [ -n "$release" ] && git rev-parse -q --verify "refs/tags/v$version" >/dev/null; then
  fail "tag v$version already exists, so v$version is already released or being released." "$version"
fi
tag=$(git tag -l 'v*' | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | sed 's/^v//' | sort -V | tail -n1 || true)
[ -z "$tag" ] || above "$version" "$tag" || fail "version $version is not above the highest release tag v$tag." "$tag"

if [ -n "$release" ]; then
  echo "version check passed: v$version is not tagged yet${tag:+ and is above v$tag}."
else
  echo "version check passed: $version is above $base's $tip${tag:+ and tag v$tag}."
fi
