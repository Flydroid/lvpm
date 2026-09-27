#!/usr/bin/env bash
# Real packages on LabVIEW for Linux, in NI's container image.
#
# Installs LUnit and G-Image from the public indexes and then, in a throwaway
# container:
#   1. a headless `lvpm install --hooks`: G-Image's PostInstall has to run for
#      its VIs to work, and every hook must have run;
#   2. every file in the install records must be on disk;
#   3. `lvpm relink --all` over the packages' folders;
#   4. `lvpm uninstall --all`, after which the LabVIEW tree must be as it was:
#      no file added, gone or changed;
#   5. the same set into a venv with `--relink`, relinked in a LabVIEW started
#      on the venv (a venv runs no hooks), which must leave the LabVIEW tree
#      alone.
#
# Whether the packages' VIs are broken is not asked: a bad VI in a package, or
# in LabVIEW's own templates, says nothing about lvpm, and a mass compile
# writes into the tree the uninstall check compares (docs/linux.md).
#
#   scripts/linux-packages.sh [image]     default: nationalinstruments/labview:latest-linux
#
# Environment:
#   LVPM         a static lvpm to use instead of building one (see linux-e2e.sh)
#   PACKAGES     space-separated package names, instead of the list below
#   SOURCE       a repository (index URL or folder) to use instead of the public indexes
#   OUT          where the logs go (default: a temporary directory, removed afterwards)
#   DOCKER_ARGS  extra `docker run` arguments, e.g. a proxy
#
# Needs Docker and network access to the package sources.
set -euo pipefail

IMAGE=${1:-nationalinstruments/labview:latest-linux}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
PACKAGES=${PACKAGES:-"astemes_lib_lunit dataflow_g_lib_g_image"}
WORK=$(mktemp -d)
NAME=lvpm-packages-$$
# The container writes into $WORK as root; it hands it back before it goes, or
# a CI runner's user could not remove it.
cleanup() {
    docker exec "$NAME" chown -R "$(id -u):$(id -g)" /work >/dev/null 2>&1 || true
    docker rm -f "$NAME" >/dev/null 2>&1 || true
    rm -rf "$WORK" || true
}
trap cleanup EXIT

if [ -n "${LVPM:-}" ]; then
    cp "$LVPM" "$WORK/lvpm"
else
    CC_x86_64_unknown_linux_musl=musl-gcc \
        cargo build --release --target x86_64-unknown-linux-musl --manifest-path "$ROOT/Cargo.toml"
    cp "$ROOT/target/x86_64-unknown-linux-musl/release/lvpm" "$WORK/lvpm"
fi
chmod +x "$WORK/lvpm"

sources=""
case "${SOURCE:-}" in
    "") ;;
    http://* | https://*) sources=$'[sources]\nsource = "'"$SOURCE"$'"\ndefaults = false\n' ;;
    *)
        cp -r "$SOURCE" "$WORK/source"
        sources=$'[sources]\nsource = "../source"\ndefaults = false\n'
        ;;
esac
for d in global venv; do
    mkdir "$WORK/$d"
    {
        printf '[project]\nname = "linux-packages-%s"\nlabview = "2026"\n\n%s\n[dependencies]\n' "$d" "$sources"
        for p in $PACKAGES; do printf '%s = "*"\n' "$p"; done
    } > "$WORK/$d/lvpm.toml"
done
mkdir "$WORK/out"

# shellcheck disable=SC2086 # DOCKER_ARGS is a list of arguments
docker run -d --init --name "$NAME" -e LV_RTE_HEADLESS=1 ${DOCKER_ARGS:-} \
    -v "$WORK:/work" "$IMAGE" sleep infinity >/dev/null
status=0
docker exec -i "$NAME" bash -s <<'SH' || status=$?
set -u
L=/work/lvpm
O=/work/out
LV=$($L targets | awk '{print $NF; exit}')
# LabVIEWCLI wants the operation first and -Headless last: in between, the flag
# takes the next argument with it, and the CLI prints its usage and exits 0.
cli() { LabVIEWCLI -OperationName "$1" -LabVIEWPath "$LV/labview" "${@:2}" -Headless; }
fails=0
check() { if eval "$2"; then echo "ok    $1"; else echo "FAIL  $1"; fails=$((fails + 1)); fi; }
tree() { find "$LV" -path "$LV/VIObjCache" -prune -o -print | sort; }
# The LabVIEW tree against snapshot $1: no name added or gone, and no file
# that was there written since. VIObjCache is LabVIEW's compile cache.
snapshot() { tree > $O/$1-before.txt; touch /tmp/$1.marker; sleep 1; }
untouched() {
    tree > $O/$1-after.txt
    diff $O/$1-before.txt $O/$1-after.txt > $O/$1-tree.diff
    find "$LV" -path "$LV/VIObjCache" -prune -o -type f -newer /tmp/$1.marker -print | sort |
        comm -12 - $O/$1-before.txt > $O/$1-changed.txt
    [ ! -s $O/$1-tree.diff ] && [ ! -s $O/$1-changed.txt ]
}
show_changes() {
    [ -s $O/$1-tree.diff ] && { echo "  added or gone:"; head -20 $O/$1-tree.diff; }
    [ -s $O/$1-changed.txt ] && { echo "  changed: $(wc -l < $O/$1-changed.txt) file(s)"; head -20 $O/$1-changed.txt; }
    true
}
close_labview() {
    cli CloseLabVIEW >> $O/close.log 2>&1 || true
    for _ in $(seq 30); do pgrep -x labview >/dev/null || break; sleep 1; done
}

snapshot global
cd /work/global
check "headless install --hooks resolves and copies every package" '$L install --hooks > $O/install.log 2>&1'
cat $O/install.log
$L list > $O/list.txt 2>/dev/null; cat $O/list.txt
asked=$(sed -n '/^\[dependencies\]/,$p' lvpm.toml | grep -oE '^[A-Za-z0-9_.-]+' || true)
missing=$(for p in $asked; do grep -qi "^$p " $O/list.txt || echo "$p"; done)
check "lvpm list shows every package asked for" '[ -n "$asked" ] && [ -z "$missing" ]'
[ -n "$missing" ] && echo "  missing: $missing"
check "every install hook ran" '! grep -q "hooks skipped" $O/list.txt'

python3 - > $O/placement.txt <<'PY'
import glob, json, os
records = [json.load(open(r)) for r in sorted(glob.glob("/var/lib/lvpm/*/installed/*.json"))]
files = [(m["name"], f) for m in records for f in m["files"]]
print(f"{len(files)} file(s) in {len(records)} install record(s)")
for name, f in files:
    if not os.path.lexists(f):
        print(f"not on disk: {name}: {f}")
PY
head -1 $O/placement.txt
check "every file in the install records is on disk" \
    '! grep -q "^0 file" $O/placement.txt && ! grep -q "^not on disk" $O/placement.txt'
grep "^not on disk" $O/placement.txt | head -20

check "relink --all over the installed packages" '$L relink --all > $O/relink.log 2>&1'
grep -E "^\s+/|FAILED|saved" $O/relink.log | head -40
check "lvpm list shows every package relinked" '! $L list 2>/dev/null | grep -q "NOT relinked"'
close_labview

check "uninstall --all" '$L uninstall --all > $O/uninstall.log 2>&1'
# The uninstall hooks can start LabVIEW; what it writes as it closes counts too.
close_labview
check "the LabVIEW tree is as it was: no file added, gone or changed" 'untouched global'
show_changes global

cd /work/venv
snapshot venv
check "venv create" '$L venv create > $O/venv.log 2>&1'
check "venv install --relink" '$L install --relink > $O/venv-install.log 2>&1'
grep -E "^\+|FAILED|relinking|skipped|not run" $O/venv-install.log | head -40
check "every venv package relinked" \
    '$L list 2>/dev/null | grep -q " relinked" && ! $L list 2>/dev/null | grep -q "NOT relinked"'
check "the venv left the LabVIEW tree alone" 'untouched venv'
show_changes venv

echo
[ "$fails" = 0 ] && echo "all checks passed" || { echo "$fails check(s) failed; logs in the OUT directory"; exit 1; }
SH
# The logs are world-readable: a copy needs no change of owner, and the OUT
# directory the caller named is left as it was.
if [ -n "${OUT:-}" ]; then
    mkdir -p "$OUT"
    cp -r "$WORK/out/." "$OUT/"
    echo "logs: $OUT"
fi
exit "$status"
