#!/usr/bin/env bash
# Writes the release assets: lvpm-<version>-windows-x64.zip,
# lvpm-<version>-linux-x64.tar.gz and SHA256SUMS (sha256sum format).
# Each archive holds the binary, README.md, LICENSE-MIT and LICENSE-APACHE at
# its top level, the layout scripts/install.ps1 expects.
# Usage: package.sh <version> <windows-exe> <linux-bin> <out-dir>  (run from the repo root)
set -euo pipefail

version="${1:?usage: package.sh <version> <windows-exe> <linux-bin> <out-dir>}"
win="${2:?missing <windows-exe>}" linux="${3:?missing <linux-bin>}" out="${4:?missing <out-dir>}"
docs=(README.md LICENSE-MIT LICENSE-APACHE)
for f in "$win" "$linux" "${docs[@]}"; do [ -f "$f" ] || { echo "package: $f not found" >&2; exit 1; }; done

stage=$(mktemp -d); trap 'rm -rf "$stage"' EXIT
mkdir -p "$out" "$stage/win" "$stage/linux"
cp "$win" "$stage/win/lvpm.exe"
cp "$linux" "$stage/linux/lvpm"
chmod 0755 "$stage/linux/lvpm"
for d in win linux; do cp "${docs[@]}" "$stage/$d/"; done

zip_name="lvpm-$version-windows-x64.zip" tar_name="lvpm-$version-linux-x64.tar.gz"
(cd "$stage/win" && zip -qX "$stage/$zip_name" lvpm.exe "${docs[@]}")
tar -czf "$stage/$tar_name" --owner=0 --group=0 -C "$stage/linux" lvpm "${docs[@]}"
mv "$stage/$zip_name" "$stage/$tar_name" "$out/"
(cd "$out" && sha256sum "$zip_name" "$tar_name" > SHA256SUMS)
echo "packaged $out/$zip_name, $out/$tar_name and $out/SHA256SUMS"
