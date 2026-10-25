#!/usr/bin/env bash
# Self-test for package.sh: packages stub binaries and checks what comes out.
set -uo pipefail
repo="$(cd "$(dirname "$0")/.." && pwd)"
work=$(mktemp -d); trap 'cd /; rm -rf "$work"' EXIT
failures=0
check() { # check <name> <command...>
  local name=$1; shift
  local out
  if out=$("$@" 2>&1); then echo "ok   $name"; else echo "FAIL $name: $out"; failures=$((failures+1)); fi
}

printf 'MZ stub' > "$work/lvpm.exe"
printf '#!/bin/sh\necho stub\n' > "$work/lvpm"
chmod 0644 "$work/lvpm" # package.sh must set the mode itself
cd "$repo"
bash ci/package.sh 9.8.7 "$work/lvpm.exe" "$work/lvpm" "$work/out" || { echo "FAIL package.sh exited $?"; exit 1; }
out="$work/out"
want=$'LICENSE-APACHE\nLICENSE-MIT\nREADME.md\nlvpm.exe'
check "zip holds the binary and the docs" test "$(unzip -Z1 "$out/lvpm-9.8.7-windows-x64.zip" | LC_ALL=C sort)" = "$want"
check "tar holds the binary and the docs" test "$(tar -tzf "$out/lvpm-9.8.7-linux-x64.tar.gz" | LC_ALL=C sort)" = "${want/lvpm.exe/lvpm}"
check "tar binary is mode 0755" bash -c "tar -tvzf '$out/lvpm-9.8.7-linux-x64.tar.gz' lvpm | grep -q '^-rwxr-xr-x'"
check "SHA256SUMS lists both archives" test "$(awk '{print $2}' "$out/SHA256SUMS")" = $'lvpm-9.8.7-windows-x64.zip\nlvpm-9.8.7-linux-x64.tar.gz'
check "SHA256SUMS verifies" bash -c "cd '$out' && sha256sum -c SHA256SUMS"
check "missing binary fails" bash -c "! bash ci/package.sh 9.8.7 '$work/nope.exe' '$work/lvpm' '$work/out2'"
check "nothing written on failure" test ! -e "$work/out2"

[ $failures = 0 ] && echo "all package scenarios passed" || { echo "$failures scenario(s) failed"; exit 1; }
