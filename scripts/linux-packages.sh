#!/usr/bin/env bash
# Real packages on LabVIEW for Linux, in NI's container image.
#
# Resolves a set of public packages that are plain G and need no NI driver
# (OpenG and the JKI State Machine), and then, in a throwaway container:
#   1. a headless `lvpm install`, which must not start LabVIEW;
#   2. a LabVIEWCLI mass compile of every folder the packages own, which must
#      report no bad VI;
#   3. `lvpm relink --all` over the same folders;
#   4. `lvpm uninstall --all`, which must leave the LabVIEW tree as it found it;
#   5. the same set into a venv with `--relink`, relinked in a LabVIEW started
#      on the venv.
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
PACKAGES=${PACKAGES:-"oglib_array oglib_boolean oglib_dictionary oglib_error oglib_file \
oglib_lvdata oglib_md5 oglib_numeric oglib_string oglib_time oglib_variantconfig \
jki_lib_state_machine"}
WORK=$(mktemp -d)
NAME=lvpm-packages-$$
KEEP_OUT=${OUT:+yes}
OUT=${OUT:-$WORK/out}
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
trap 'docker rm -f "$NAME" >/dev/null 2>&1 || true; rm -rf "$WORK"' EXIT

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

# shellcheck disable=SC2086 # DOCKER_ARGS is a list of arguments
docker run -d --init --name "$NAME" -e LV_RTE_HEADLESS=1 ${DOCKER_ARGS:-} \
    -v "$WORK:/work" -v "$OUT:/out" "$IMAGE" sleep infinity >/dev/null
status=0
docker exec -i "$NAME" bash -s <<'SH' || status=$?
set -u
L=/work/lvpm
LV=$($L targets | awk '{print $NF; exit}')
# LabVIEWCLI wants the operation first and -Headless last: in between, the flag
# takes the next argument with it, and the CLI prints its usage and exits 0.
cli() { LabVIEWCLI -OperationName "$1" -LabVIEWPath "$LV/labview" "${@:2}" -Headless; }
fails=0
check() { if eval "$2"; then echo "ok    $1"; else echo "FAIL  $1"; fails=$((fails + 1)); fi; }
tree() { find "$LV" -path "$LV/VIObjCache" -prune -o -print | sort; }

tree > /out/tree-before.txt
cd /work/global
check "headless install resolves and copies every package" '$L install > /out/install.log 2>&1'
cat /out/install.log
check "a headless install starts no LabVIEW" '! pgrep -x labview >/dev/null'
$L list > /out/list.txt 2>/dev/null; cat /out/list.txt
asked=$(sed -n '/^\[dependencies\]/,$p' lvpm.toml | grep -oE '^[A-Za-z0-9_.-]+' || true)
missing=$(for p in $asked; do grep -q "^$p " /out/list.txt || echo "$p"; done)
check "lvpm list shows every package asked for" '[ -n "$asked" ] && [ -z "$missing" ]'
[ -n "$missing" ] && echo "  missing: $missing"

# Every folder the packages own, nested ones folded into their parent: the
# same set the relink pass walks.
python3 - > /work/folders.txt <<'PY'
import glob, json
dirs = sorted({d for f in glob.glob("/var/lib/lvpm/*/installed/*.json") for d in json.load(open(f))["relink_folders"]})
print("\n".join(d for d in dirs if not any(d.startswith(p + "/") for p in dirs)))
PY
echo "mass compiling $(wc -l < /work/folders.txt) folder(s)"
n=0; mc_ok=yes
while read -r d; do
    n=$((n + 1)); log=/out/masscompile-$n.log
    cli MassCompile -DirectoryToCompile "$d" -MassCompileLogFile "$log" > "/out/masscompile-$n.out" 2>&1
    if grep -q "MassCompile operation succeeded" "/out/masscompile-$n.out" && [ -f "$log" ] && ! grep -q "Bad VI" "$log"; then
        echo "  $d ... ok"
    else
        mc_ok=no
        echo "  $d ... FAILED"
        { [ -f "$log" ] && grep -E "Bad VI|Search failed" "$log" || tail -5 "/out/masscompile-$n.out"; } |
            sort -u | head -12 | sed 's/^ */      /'
    fi
done < /work/folders.txt
check "mass compile reports no bad VI" '[ $mc_ok = yes ] && [ $n -gt 0 ]'

check "relink --all over the installed packages" '$L relink --all > /out/relink.log 2>&1'
grep -E "^\s+/|FAILED|saved" /out/relink.log | head -40
check "lvpm list shows every package relinked" '! $L list 2>/dev/null | grep -q "NOT relinked"'
cli CloseLabVIEW > /out/close.log 2>&1 || true
for _ in $(seq 30); do pgrep -x labview >/dev/null || break; sleep 1; done

check "uninstall --all" '$L uninstall --all > /out/uninstall.log 2>&1'
tree > /out/tree-after.txt
check "the LabVIEW tree is as it was before the install" 'diff /out/tree-before.txt /out/tree-after.txt > /out/tree.diff'
[ -s /out/tree.diff ] && head -20 /out/tree.diff

cd /work/venv
check "venv create" '$L venv create > /out/venv.log 2>&1'
check "venv install --relink" '$L install --relink > /out/venv-install.log 2>&1'
grep -E "^\+|FAILED|relinking|skipped" /out/venv-install.log | head -40
check "every venv package relinked" \
    '$L list 2>/dev/null | grep -q " relinked" && ! $L list 2>/dev/null | grep -q "NOT relinked"'

echo
[ "$fails" = 0 ] && echo "all checks passed" || { echo "$fails check(s) failed; logs in the OUT directory"; exit 1; }
SH
[ -n "$KEEP_OUT" ] && echo "logs: $OUT"
exit "$status"
