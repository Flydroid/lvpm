#!/bin/sh
# Download and install the latest lvpm release for Linux x64.
#
# Resolves the release on GitHub, downloads the static (musl) tarball over
# HTTPS, verifies its SHA-256, and installs lvpm with relink-package.vi next
# to it: lvpm hands that VI to LabVIEW for --relink and refuses one from
# another release, so the two always go together.
#
# The download is verified against every reference available before
# anything is extracted:
#   - the SHA-256 digest GitHub records for the release asset,
#   - the release's <asset>.sha256 or sha256.sum file,
#   - a hash you pass with --sha256, from a source other than GitHub.
# If none of those is available the script stops. The only thing it runs
# from the download is `lvpm --version` at the end.
#
# Needs only curl, tar (with gzip), sed, awk and sha256sum (or shasum); no
# jq, no root. Mirrors scripts/install.ps1.
#
#   curl -fsSLO https://raw.githubusercontent.com/labview-package-manager/lvpm/main/scripts/install.sh
#   sh install.sh
#
# Options:
#   --version <tag>      release to install, e.g. v0.3.1 (default: the newest)
#   --stable             skip pre-releases when picking the newest
#   --install-dir <dir>  default: /usr/local/bin as root, else ~/.local/bin
#   --sha256 <hash>      expected SHA-256 of the tarball, obtained out of band
#   --repo <owner/name>  default: labview-package-manager/lvpm
#   --archive <file>     install a local release tarball (needs --sha256)
#   --build-local        cargo build --release --locked this checkout and
#                        install that, for testing; nothing is downloaded
#   -h, --help

set -eu

repo='labview-package-manager/lvpm'
version=''
stable=0
install_dir=''
sha256=''
archive=''
build_local=0
asset='lvpm-x86_64-unknown-linux-musl.tar.gz'
vi='relink-package.vi'

say() { printf '%s\n' "$*"; }
die() { printf 'install.sh: %s\n' "$*" >&2; exit 1; }
usage() { sed -n '2,/^$/{s/^# \{0,1\}//;p}' "$0"; exit 0; }

while [ $# -gt 0 ]; do
    case "$1" in
        --version) version=${2:?--version needs a tag}; shift ;;
        --stable) stable=1 ;;
        --install-dir) install_dir=${2:?--install-dir needs a directory}; shift ;;
        --sha256) sha256=${2:?--sha256 needs a hash}; shift ;;
        --repo) repo=${2:?--repo needs owner/name}; shift ;;
        --archive) archive=${2:?--archive needs a file}; shift ;;
        --build-local) build_local=1 ;;
        -h|--help) usage ;;
        *) die "unknown option $1 (see --help)" ;;
    esac
    shift
done

case "$sha256" in
    '') ;;
    *[!0-9a-fA-F]*) die "--sha256 must be 64 hex digits" ;;
    *) [ ${#sha256} -eq 64 ] || die "--sha256 must be 64 hex digits" ;;
esac
case "$repo" in
    */*) ;;
    *) die "--repo must be owner/name" ;;
esac
if [ -n "$archive" ] || [ "$build_local" -eq 1 ]; then
    [ -z "$version" ] && [ "$stable" -eq 0 ] ||
        die "--archive and --build-local cannot be combined with --version or --stable"
fi
[ "$build_local" -eq 1 ] && [ -n "$archive$sha256" ] &&
    die "--build-local cannot be combined with --archive or --sha256"
[ -n "$archive" ] && [ -z "$sha256" ] && die "--archive requires --sha256"

[ "$(uname -s)" = Linux ] || die "this installs the Linux build; on Windows use scripts/install.ps1"
[ "$build_local" -eq 1 ] || [ "$(uname -m)" = x86_64 ] ||
    die "releases are built for x86_64 only, not $(uname -m); build from source (--build-local)"

if [ -z "$install_dir" ]; then
    if [ "$(id -u)" -eq 0 ]; then install_dir=/usr/local/bin; else install_dir="$HOME/.local/bin"; fi
fi

work=$(mktemp -d "${TMPDIR:-/tmp}/lvpm-install.XXXXXX")
# The cleanup on any exit; an interrupt also ends the script, rather than
# going on without $work.
trap 'rm -rf "$work"' EXIT
trap 'exit 130' INT TERM

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d' ' -f1
    else
        shasum -a 256 "$1" | cut -d' ' -f1
    fi
}

# Put lvpm and the VI from directory $1 into $install_dir. Each lands under
# a temporary name and is renamed over the old one, so a running lvpm keeps
# its file and nothing ever sees half of one.
install_from() {
    [ -f "$1/lvpm" ] || die "no lvpm in $1"
    [ -f "$1/$vi" ] || die "no $vi beside lvpm in $1"
    mkdir -p "$install_dir"
    cp "$1/lvpm" "$install_dir/.lvpm.$$" && chmod 755 "$install_dir/.lvpm.$$"
    cp "$1/$vi" "$install_dir/.$vi.$$" && chmod 644 "$install_dir/.$vi.$$"
    mv -f "$install_dir/.$vi.$$" "$install_dir/$vi"
    mv -f "$install_dir/.lvpm.$$" "$install_dir/lvpm"
    say "install: $install_dir"
}

finish() {
    case ":$PATH:" in
        *":$install_dir:"*) say "path:    $install_dir is on PATH" ;;
        *) say "path:    $install_dir is not on PATH; add it, e.g. in ~/.profile:"
           say "         export PATH=\"$install_dir:\$PATH\"" ;;
    esac
    say ''
    # Assigned first: set -e does not see a failure inside an argument.
    installed=$("$install_dir/lvpm" --version) || die "the installed lvpm does not run: $install_dir/lvpm --version failed"
    say "$installed installed"
}

# ---------------------------------------------------------- build-local ----

if [ "$build_local" -eq 1 ]; then
    root=$(cd "$(dirname "$0")/.." && pwd)
    cargo build --release --locked --manifest-path "$root/Cargo.toml"
    target=$(cargo metadata --no-deps --format-version 1 --manifest-path "$root/Cargo.toml" |
        sed 's/.*"target_directory":"\([^"]*\)".*/\1/')
    mkdir "$work/payload"
    cp "$target/release/lvpm" "$work/payload/"
    cp "$root/src/lv-src/$vi" "$work/payload/"
    install_from "$work/payload"
    finish
    exit 0
fi

# ------------------------------------------------------ resolve release ----

api="https://api.github.com/repos/$repo"
# -L: GitHub answers a renamed repository with a redirect.
fetch() { curl -fsSL --proto '=https' --tlsv1.2 -H 'User-Agent: lvpm-install.sh' "$@"; }
# One JSON key per line, whatever GitHub's formatting, so sed and awk can read
# the few keys needed without jq.
api_get() {
    # Through a file, not a pipe, so a failed request fails the call.
    fetch -H 'Accept: application/vnd.github+json' -H 'X-GitHub-Api-Version: 2022-11-28' \
        -o "$work/api.json" "$api/$1" || return 1
    tr ',{}[]' '\n\n\n\n\n' < "$work/api.json"
}
value() { sed -n "s/^ *\"$1\": *\"\{0,1\}\([^\"]*\)\"\{0,1\} *$/\1/p"; }

if [ -n "$archive" ]; then
    [ -f "$archive" ] || die "$archive is not a file"
    tag=local
    cp "$archive" "$work/$asset"
else
    if [ -z "$version" ]; then
        # The newest release that is not a draft (and, with --stable, not a
        # pre-release). tag_name, draft and prerelease are release-level keys;
        # assets carry none of them.
        version=$(api_get 'releases?per_page=30' | awk -v stable="$stable" '
            /^ *"tag_name":/ { if (tag != "" && ok()) { print tag; exit } ; tag = $0; sub(/^ *"tag_name": *"/, "", tag); sub(/".*/, "", tag); draft = pre = "" }
            /^ *"draft":/ { draft = $0 ~ /true/ }
            /^ *"prerelease":/ { pre = $0 ~ /true/ }
            function ok() { return !draft && !(stable && pre) }
            END { if (tag != "" && ok()) print tag }' | head -n 1)
        [ -n "$version" ] || die "no release found in $repo$( [ "$stable" -eq 1 ] && printf ' (every release is a pre-release; drop --stable)')"
    fi
    tag=$version
    release=$(api_get "releases/tags/$tag") || die "no release $tag in $repo"
    printf '%s\n' "$release" | value name | grep -qx "$asset" ||
        die "release $tag has no $asset (Linux releases start at v0.3.1)"
    pre=$(printf '%s\n' "$release" | value prerelease | head -n 1)
    say "release: $tag$( [ "$pre" = true ] && printf ' (pre-release)')"
    say "asset:   $asset"
    fetch -o "$work/$asset" "https://github.com/$repo/releases/download/$tag/$asset"
fi

# --------------------------------------------------------------- verify ----

actual=$(sha256_of "$work/$asset")
checked=0
check() { # source, expected hash
    expected=$(printf '%s' "$2" | tr 'A-F' 'a-f')
    [ "$expected" = "$actual" ] ||
        die "SHA-256 mismatch against $1: expected $expected, got $actual. Nothing was installed."
    say "sha256:  ok ($1)"
    checked=$((checked + 1))
}

[ -n "$sha256" ] && check '--sha256' "$sha256"
if [ -z "$archive" ]; then
    # The digest GitHub records: it follows the asset's name in its entry.
    digest=$(printf '%s\n' "$release" | awk -v a="$asset" '
        /^ *"name":/ { mine = ($0 ~ "\"" a "\"") }
        mine && /^ *"digest": *"sha256:/ { sub(/.*sha256:/, ""); sub(/".*/, ""); print; exit }')
    [ -n "$digest" ] && check 'GitHub asset digest' "$digest"
    for sums in "$asset.sha256" sha256.sum; do
        if printf '%s\n' "$release" | value name | grep -qx "$sums" &&
            fetch -o "$work/$sums" "https://github.com/$repo/releases/download/$tag/$sums"; then
            hash=$(awk -v a="$asset" '{ n = $2; sub(/^\*/, "", n) } n == a { print $1; exit }' "$work/$sums")
            [ -n "$hash" ] && check "$sums" "$hash"
        fi
    done
fi
[ "$checked" -gt 0 ] ||
    die "nothing to verify $asset against: no digest and no checksum file. Pass --sha256 <hash> from a source you trust, or stop here."

# -------------------------------------------------------------- install ----

mkdir "$work/unpacked"
tar -xzf "$work/$asset" -C "$work/unpacked"
dir=$(dirname "$(find "$work/unpacked" -type f -name lvpm | head -n 1)")
[ "$dir" != . ] || die "$asset contains no lvpm"
install_from "$dir"
finish
