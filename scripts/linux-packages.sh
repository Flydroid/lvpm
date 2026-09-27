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
# The container writes into the mounted directories as root; it hands them back
# before it goes, or a CI runner's user could not remove them.
cleanup() {
    docker exec "$NAME" chown -R "$(id -u):$(id -g)" /work /out >/dev/null 2>&1 || true
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
# Packages with something to relink that are not marked relinked. One that
# installed no VI, library or LLB has nothing to relink and stays unmarked.
unrelinked() {
    python3 - "$1" <<'PY'
import glob, json, sys
for f in sorted(glob.glob(sys.argv[1] + "/*.json")):
    m = json.load(open(f))
    if m["relink_folders"] and not m["relinked"]:
        print(m["name"])
PY
}
# The bad VIs in a mass-compile log that a package installed, each once (the
# log names a VI again for every caller that loads it); a VI inside an LLB
# counts for the LLB. Folders a package shares with LabVIEW hold LabVIEW's own
# files too, and those are not what this checks.
ours() {
    python3 - "$1" <<'PY'
import re, sys
installed = set(open("/work/installed.txt").read().split("\n")) - {""}
seen = []
for path in re.findall(r'Path="([^"]*)"', open(sys.argv[1], errors="replace").read()):
    p = path
    while p not in installed and "/" in p:
        p = p.rsplit("/", 1)[0]
    if p in installed and path not in seen:
        seen.append(path)
print("\n".join(seen))
PY
}
# What uninstall left behind or took away. LabVIEW writes .aliases, .lvlps and
# .UserState files beside a project or library it opens; those are its own.
leftovers() {
    python3 - <<'PY'
import os
before = set(open("/out/tree-before.txt").read().split("\n"))
after = set(open("/out/tree-after.txt").read().split("\n"))
side = lambda p: p.endswith((".aliases", ".lvlps")) or ".UserState" in p
def only_side(d):
    files = [os.path.join(r, f) for r, _, fs in os.walk(d) for f in fs]
    return files and all(side(f) for f in files)
for p in sorted(after - before):
    if not side(p) and not (os.path.isdir(p) and only_side(p)):
        print("left behind", p)
for p in sorted(before - after - {""}):
    print("taken away ", p)
PY
}

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

# Every folder the packages own, nested ones folded into their parent (the set
# the relink pass walks), and every file they installed.
python3 - <<'PY'
import glob, json
ms = [json.load(open(f)) for f in glob.glob("/var/lib/lvpm/*/installed/*.json")]
dirs = sorted({d for m in ms for d in m["relink_folders"]})
open("/work/folders.txt", "w").write("".join(d + "\n" for d in dirs if not any(d.startswith(p + "/") for p in dirs)))
open("/work/installed.txt", "w").write("".join(f + "\n" for m in ms for f in m["files"]))
PY
echo "mass compiling $(wc -l < /work/folders.txt) folder(s)"
n=0; ran=yes; bad_total=0
while read -r d; do
    n=$((n + 1)); log=/out/masscompile-$n.log; out=/out/masscompile-$n.out
    cli MassCompile -DirectoryToCompile "$d" -MassCompileLogFile "$log" > "$out" 2>&1
    if ! grep -q "MassCompile operation" "$out" || [ ! -f "$log" ]; then
        ran=no
        echo "  $d ... LabVIEWCLI did not compile it"
        tail -5 "$out" | sed 's/^/      /'
        continue
    fi
    bad=$(ours "$log")
    count=$(printf '%s' "$bad" | grep -c . || true)
    others=$(( $(grep -c "Bad VI" "$log" || true) - count ))
    note=""; [ "$others" -gt 0 ] && note=" ($others bad VI(s) of LabVIEW's own in this folder, not counted)"
    if [ "$count" = 0 ]; then
        echo "  $d ... ok$note"
    else
        bad_total=$((bad_total + count))
        echo "  $d ... $count bad VI(s)$note"
        printf '%s\n' "$bad" | head -10 | sed "s|^$LV/|      |"
        [ "$count" -gt 10 ] && echo "      ... and $((count - 10)) more (masscompile-$n.log)"
    fi
done < /work/folders.txt
check "mass compile finds no bad VI among the packages' files" '[ $ran = yes ] && [ $n -gt 0 ] && [ $bad_total = 0 ]'

check "relink --all over the installed packages" '$L relink --all > /out/relink.log 2>&1'
grep -E "^\s+/|FAILED|saved|nothing to relink" /out/relink.log | head -40
check "every package with something to relink is relinked" '[ -z "$(unrelinked "/var/lib/lvpm/*/installed")" ]'
cli CloseLabVIEW > /out/close.log 2>&1 || true
for _ in $(seq 30); do pgrep -x labview >/dev/null || break; sleep 1; done

check "uninstall --all" '$L uninstall --all > /out/uninstall.log 2>&1'
tree > /out/tree-after.txt
leftovers > /out/leftovers.txt
check "uninstall leaves the LabVIEW tree as it was" '[ ! -s /out/leftovers.txt ]'
head -20 /out/leftovers.txt
side=$(comm -13 /out/tree-before.txt /out/tree-after.txt | grep -cE '\.aliases$|\.lvlps$|\.UserState' || true)
[ "$side" -gt 0 ] && echo "  note: $side project file(s) LabVIEW wrote itself are still there"

cd /work/venv
check "venv create" '$L venv create > /out/venv.log 2>&1'
check "venv install --relink" '$L install --relink > /out/venv-install.log 2>&1'
grep -E "^\+|FAILED|relinking|skipped" /out/venv-install.log | head -40
check "every venv package with something to relink is relinked" \
    '[ -n "$(ls .project/.lvpm/installed)" ] && [ -z "$(unrelinked .project/.lvpm/installed)" ]'

echo
[ "$fails" = 0 ] && echo "all checks passed" || { echo "$fails check(s) failed; logs in the OUT directory"; exit 1; }
SH
[ -n "$KEEP_OUT" ] && echo "logs: $OUT"
exit "$status"
