#!/usr/bin/env bash
# Self-test for version-check.sh: one throwaway repo per scenario.
set -uo pipefail
script="$(cd "$(dirname "$0")" && pwd)/version-check.sh"
root=$(mktemp -d); trap 'cd /; rm -rf "$root"' EXIT # Windows can't delete the cwd
failures=0; n=0
export GITHUB_STEP_SUMMARY="$root/summary" # never write into the CI job's own summary

write() { # write <toml version or empty> [<lock version>] [<lock-core version>]
  { echo '[workspace.package]'; [ -n "$1" ] && echo "version = \"$1\""
    printf '\n[package]\nname = "lvpm"\nversion.workspace = true\n'; } > Cargo.toml
  printf '[[package]]\nname = "lvpm"\nversion = "%s"\n\n[[package]]\nname = "lvpm-core"\nversion = "%s"\n' \
    "${2-$1}" "${3-${2-$1}}" > Cargo.lock
}
commit() { git add -A && git commit -qm "$1"; }
setup() { # setup <main version> [tags...]: a repo on branch pr forked from main
  dir="$root/$((++n))"; mkdir "$dir"; cd "$dir"
  git init -q -b main && git config core.autocrlf false && git config user.email t@t && git config user.name t \
    && git config commit.gpgsign false && git config tag.gpgSign false
  write "$1"; commit main; shift
  for t in "$@"; do git tag "$t"; done
  git checkout -qb pr
}
expect() { # expect <0|1> <name> [<substring>...]: run the check on branch pr against main
  local want=$1 name=$2; shift 2
  local out; out=$(bash "$script" "${mode:-main}" 2>&1); local got=$?
  local ok=1; [ "$got" = "$want" ] || ok=0
  for s in "$@"; do [[ "$out" == *"$s"* ]] || ok=0; done
  if [ $ok = 1 ]; then echo "ok   $name"; else echo "FAIL $name (exit $got): $out"; failures=$((failures+1)); fi
}
pr() { write "$@"; commit pr; }
stray() { printf '[dependencies.foo]\nversion = "9.9.9"\n\n' | cat - Cargo.toml > t && mv t Cargo.toml && commit stray; }

for v in 0.2.2 0.3.0 1.0.0; do setup 0.2.1 v0.2.1; pr $v; expect 0 "higher $v passes"; done
setup 0.2.1 v0.2.1; expect 1 "unchanged fails" 0.2.1 'Raise `[workspace.package] version`' 'cargo update -w'
setup 0.2.1 v0.2.1; pr 0.2.0; expect 1 "lower fails" 0.2.0 0.2.1
setup 0.2.0 v0.3.0; pr 0.3.0; expect 1 "equal to tag fails" 0.3.0 v0.3.0
setup 0.2.1 v0.4.0; pr 0.3.0; expect 1 "below tag fails" 0.3.0 v0.4.0
setup 0.2.1 v0.2.1; pr 0.3.0; git checkout -q main; write 0.3.0; commit other; git checkout -q pr
expect 1 "main moved fails" "main is now 0.3.0" "rebase and raise above 0.3.0"
setup 0.2.1; pr 0.2.2; expect 0 "no tags passes"
setup 0.2.1 v0.2.1 v0.9.0-rc.1 nightly; pr 0.3.0; expect 0 "non-plain tags ignored"
for v in 0.3.0-rc.1 0.3.0+ci 0.3 0.3.0.1 v0.3.0 0.03.0; do
  setup 0.2.1 v0.2.1; pr $v; expect 1 "malformed $v fails" "must be MAJOR.MINOR.PATCH"
done
setup 0.2.1 v0.2.1; pr ""; expect 1 "missing version fails" "no workspace version was found"
setup 0.2.1 v0.2.1; pr 0.3.0 0.2.1 0.3.0; expect 1 "stale lvpm lock fails" "lvpm 0.2.1" 0.3.0
setup 0.2.1 v0.2.1; pr 0.3.0 0.3.0 0.2.1; expect 1 "stale lvpm-core lock fails" "lvpm-core 0.2.1" 0.3.0
setup 0.9.0 v0.9.0; pr 0.10.0; expect 0 "numeric compare passes"
setup 0.2.1 v0.2.1; pr 0.3.0; stray; expect 0 "version outside [workspace.package] ignored"
setup 0.2.1 v0.2.1; pr ""; stray; expect 1 "only a stray version fails" "no workspace version was found"
setup 0.2.1 v0.2.1; pr 0.3.0; rm Cargo.lock; commit nolock; expect 1 "missing lock fails" "Cargo.lock is missing"
setup 0.2.1 v0.2.1; pr 0.3.0; git checkout -q main; git rm -q Cargo.toml; commit nobase; git checkout -q pr
expect 1 "unreadable base fails" "could not read main:Cargo.toml"

mode=--release
setup 0.2.1 v0.2.1; pr 0.3.0; expect 0 "release of an untagged version passes" "v0.3.0 is not tagged yet"
setup 0.3.0; expect 0 "release without a base comparison passes"
setup 0.2.1 v0.3.0; pr 0.3.0; expect 1 "release of an existing tag fails" "tag v0.3.0 already exists" "The next release PR must carry a version above 0.3.0."
setup 0.2.1 v0.4.0; pr 0.3.0; expect 1 "release below the highest tag fails" 0.3.0 v0.4.0 "next release PR"
setup 0.2.1 v0.2.1; pr 0.3.0 0.2.1 0.3.0; expect 1 "release with a stale lock fails" "lvpm 0.2.1"
: > "$GITHUB_STEP_SUMMARY"
setup 0.2.1 v0.3.0; pr 0.3.0; expect 1 "release failure is written to the job summary"
if grep -qF "tag v0.3.0 already exists" "$GITHUB_STEP_SUMMARY" && grep -qF "next release PR" "$GITHUB_STEP_SUMMARY"
then echo "ok   job summary holds the failure and the next action"
else echo "FAIL job summary: $(cat "$GITHUB_STEP_SUMMARY")"; failures=$((failures+1)); fi
mode=--print-version
setup 0.2.1; pr 0.3.0
if [ "$(bash "$script" --print-version)" = 0.3.0 ]; then echo "ok   print-version prints exactly the version"
else echo "FAIL print-version printed '$(bash "$script" --print-version)'"; failures=$((failures+1)); fi
mode=

for row in 0.3.0:true 0.99.99:true 1.0.0:false; do
  got=$(bash "$script" --print-prerelease "${row%%:*}")
  if [ "$got" = "${row#*:}" ]; then echo "ok   prerelease ${row%%:*} -> $got"
  else echo "FAIL prerelease ${row%%:*}: got $got"; failures=$((failures+1)); fi
done

[ $failures = 0 ] && echo "all version-check scenarios passed" || { echo "$failures scenario(s) failed"; exit 1; }
