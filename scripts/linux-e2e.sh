#!/usr/bin/env bash
# End-to-end check of lvpm against LabVIEW for Linux, in NI's container image.
#
# Builds a static lvpm, makes two test packages from the repo's own VIs in
# tools/, and drives every LabVIEW-facing path in a throwaway container:
# detection, a headless global install with relink and hooks, palette and menu
# refresh, relink, uninstall, and a venv install whose overlay is proven by a
# VI that links into <vilib> running on the venv's LabVIEW and not on the
# plain one. No package index is contacted; everything comes from a local
# folder. See docs/linux.md for what each step is evidence of.
#
#   scripts/linux-e2e.sh [image]     default: nationalinstruments/labview:latest-linux
#
# Needs Docker, rustup's x86_64-unknown-linux-musl target, musl-gcc, python3;
# with LVPM set to a static lvpm, only Docker and python3.
set -euo pipefail

IMAGE=${1:-nationalinstruments/labview:latest-linux}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
WORK=$(mktemp -d)
NAME=lvpm-e2e-$$
# The container writes into $WORK as root; it hands it back before it goes, or
# a CI runner's user could not remove it.
cleanup() {
    docker exec "$NAME" chown -R "$(id -u):$(id -g)" /work >/dev/null 2>&1 || true
    docker rm -f "$NAME" >/dev/null 2>&1 || true
    rm -rf "$WORK" || true
}
trap cleanup EXIT

# NI's image is Ubuntu 22.04 (glibc 2.35); a static binary runs there whatever
# the build host's glibc.
if [ -n "${LVPM:-}" ]; then
    cp "$LVPM" "$WORK/lvpm"
else
    CC_x86_64_unknown_linux_musl=musl-gcc \
        cargo build --release --target x86_64-unknown-linux-musl --manifest-path "$ROOT/Cargo.toml"
    cp "$ROOT/target/x86_64-unknown-linux-musl/release/lvpm" "$WORK/lvpm"
fi
chmod +x "$WORK/lvpm"
# In a folder of its own: LabVIEW searches a missing subVI under the top-level
# VI's folder, and the venv's copy must not be found that way.
mkdir "$WORK/caller"
cp "$ROOT/tools/Test - Relink Packages E2E.vi" "$WORK/caller/caller.vi"

python3 - "$ROOT/tools" "$WORK" <<'PY'
import sys, zipfile, os
tools, work = sys.argv[1], sys.argv[2]
os.makedirs(f"{work}/pkgs")

def vip(name, spec, members):
    with zipfile.ZipFile(f"{work}/pkgs/{name}-1.0.0.1.vip", "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr("spec", f'[Package]\nName="{name}"\nVersion="1.0.0.1"\n\n' + spec)
        for arc, src in members:
            if src is None:
                z.writestr(arc, "lvpm Linux test\n")
            else:
                z.write(f"{tools}/{src}", arc)

# Files into <vi.lib> and one <OS ...> location, plus both install hooks.
vip("lvpm_linux_test", '''[Script VIs]
PreInstall="PreInstall.vi"
PostInstall="PostInstall.vi"

[File Group 0]
Target Dir="<vi.lib>/lvpm_linux_test"
Replace Mode="Always"
Num Files=3
File 0="Relink Packages.vi"
File 1="Test - Relink Packages E2E.vi"
File 2="E2E Call VI.vi"

[File Group 1]
Target Dir="<OS User Documents>/lvpm_linux_test"
Replace Mode="Always"
Num Files=1
File 0="readme.txt"
''', [
    ("File Group 0/Relink Packages.vi", "Relink Packages.vi"),
    ("File Group 0/Test - Relink Packages E2E.vi", "Test - Relink Packages E2E.vi"),
    ("File Group 0/E2E Call VI.vi", "E2E Call VI.vi"),
    ("File Group 1/readme.txt", None),
    ("PreInstall.vi", "E2E Call VI.vi"),
    ("PostInstall.vi", "E2E Call VI.vi"),
])

# A caller and its subVI in two folders: what the relink walk must find. The
# second Target Dir is spelled with backslashes, as some specs do.
vip("lvpm_linux_relink_test", '''[File Group 0]
Target Dir="<vi.lib>/lvpm_linux_relink_test"
Replace Mode="Always"
Num Files=1
File 0="Test - Relink Packages E2E.vi"

[File Group 1]
Target Dir="<vi.lib>\\lvpm_linux_relink_test\\sub"
Replace Mode="Always"
Num Files=1
File 0="Relink Packages.vi"
''', [
    ("File Group 0/Test - Relink Packages E2E.vi", "Test - Relink Packages E2E.vi"),
    ("File Group 1/Relink Packages.vi", "Relink Packages.vi"),
])

for d, name in (("global", "linux-e2e-global"), ("venv", "linux-e2e-venv")):
    os.makedirs(f"{work}/{d}")
    open(f"{work}/{d}/lvpm.toml", "w").write(f'''[project]
name = "{name}"
labview = "2026"

[sources]
local = "../pkgs"
defaults = false

[dependencies]
lvpm_linux_test = "*"
''')
PY

docker run -d --init --name "$NAME" -e LV_RTE_HEADLESS=1 -v "$WORK:/work" "$IMAGE" sleep infinity >/dev/null
docker exec -i "$NAME" bash -s <<'SH'
set -u
L=/work/lvpm
LV=$($L targets | awk '{print $NF; exit}')
fails=0
check() { if eval "$2"; then echo "ok    $1"; else echo "FAIL  $1"; fails=$((fails + 1)); fi; }
listening() { awk 'NR>1 && $4=="0A" {print $2}' /proc/net/tcp | grep -qi ":$(printf '%04X' "$1")$"; }
run() { echo "\$ lvpm $*" >> /work/log; "$L" "$@" >> /work/log 2>&1; }

check "targets: detects LabVIEW 2026 with its minor version" \
    '$L targets | grep -Eq "LabVIEW 2026 \(64-bit\) +v26\.[0-9] .*/usr/local/natinst/LabVIEW-2026-64"'

cd /work/global
check "headless global install with --relink --hooks" 'run install --relink --hooks'
check "files in vi.lib and in <OS User Documents> (\$HOME/Documents)" \
    '[ -f "$LV/vi.lib/lvpm_linux_test/E2E Call VI.vi" ] && [ -f "$HOME/Documents/lvpm_linux_test/readme.txt" ]'
check "both hooks ran" 'grep -q "pre-install ok" /work/log && grep -Eq "lvpm_linux_test \.\.\. ok" /work/log'
check "install records kept in /var/lib/lvpm" '[ -f /var/lib/lvpm/LabVIEW-2026-64bit/installed/lvpm_linux_test.json ]'
check "a backslashed Target Dir lands in real subfolders" \
    'run install lvpm_linux_relink_test --relink && [ -f "$LV/vi.lib/lvpm_linux_relink_test/sub/Relink Packages.vi" ]'
check "list shows both relinked" '[ "$($L list 2>/dev/null | grep -v "NOT relinked" | grep -c relinked)" = 2 ]'
check "run-hooks reruns a PostInstall" 'run run-hooks lvpm_linux_test --labview-version 2026'

VI=$(ls "$HOME"/.cache/lvpm/lvpm-relink-package-*.vi)
check "relink VI walks a Linux folder (finds caller and subVI)" \
    '$L vi-run "$VI" --labview-version 2026 --set "Folder to relink=str:$LV/vi.lib/lvpm_linux_relink_test" --get "all lv items" 2>&1 | grep -q "sub/Relink Packages.vi"'
out=$($L refresh --labview-version 2026 2>&1)
check "refresh: both LabVIEW refresh VIs found and run" \
    'grep -Eq "palettes +ok" <<<"$out" && grep -Eq "menus +ok" <<<"$out"'
check "relink --all" 'run relink --all'
check "vi-save of a caller relinks it to <vilib>" 'run vi-save /work/caller/caller.vi --labview-version 2026'
check "uninstall --all leaves vi.lib clean" \
    'run uninstall --all && ! ls "$LV/vi.lib" | grep -q lvpm_ && [ ! -e "$HOME/Documents/lvpm_linux_test" ]'

# The plain LabVIEW has had the package's VIs in memory, and a caller links a
# subVI of the same name that is still loaded; only a fresh one is a fair
# control for the overlay check below.
pkill -x labview; for _ in $(seq 30); do pgrep -x labview >/dev/null || break; sleep 1; done

cd /work/venv
check "venv create" 'run venv create'
check "venv install with relink in a LabVIEW started with -pref" 'run install --relink'
check "venv ini mounts the venv with forward slashes" \
    'grep -qx "LVAddons.AdditionalLocations=/work/venv/.project" .project/.lvpm/labview.ini'
PORT=$(python3 -c 'import json; print(json.load(open(".project/.lvpm/venv.json"))["port"])')
run launch
for _ in $(seq 60); do listening "$PORT" && break; sleep 2; done
check "launch straight after install still gets VI Server (TIME_WAIT)" 'listening "$PORT"'
for _ in $(seq 30); do $L vi-probe "$LV/vi.lib/Utility/error.llb/General Error Handler.vi" >/dev/null 2>&1 && break; sleep 2; done
check "overlay: caller linked into <vilib> runs on the venv's LabVIEW" \
    '$L vi-run /work/caller/caller.vi --timeout 60 >/dev/null 2>&1'
check "control: the same caller is broken on a fresh plain LabVIEW" \
    'run start --global --labview-version 2026 &&
     $L vi-run /work/caller/caller.vi --global --labview-version 2026 --timeout 60 2>&1 | grep -q "error 1003"'

echo
[ "$fails" = 0 ] && echo "all checks passed" || { echo "$fails check(s) failed; lvpm's output:"; cat /work/log; exit 1; }
SH
