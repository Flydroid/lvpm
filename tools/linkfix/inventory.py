#!/usr/bin/env python3
r"""Inventory the files lvpm installed, and diff install states.

Reads lvpm's install records (see lvpmdata.py) and records what is actually on
disk. Use it to capture a known-good install before changing it -- a linkfix
run, a LabVIEW save -- then see exactly which files moved.

    # capture the current state of a venv (every package, or only some)
    python tools/linkfix/inventory.py snapshot C:\Git\venv-check -o before.json
    python tools/linkfix/inventory.py snapshot C:\Git\venv-check jki_lib_serialization -o before.json

    # after changing it
    python tools/linkfix/inventory.py snapshot C:\Git\venv-check -o after.json
    python tools/linkfix/inventory.py diff before.json after.json

    # what changed on disk, vs the raw bytes in lvpm's cached archive?
    python tools/linkfix/inventory.py vs-archive C:\Git\venv-check jki_lib_serialization

`vs-archive` is the interesting one: an archive is a plain zip whose payload
lives under "File Group N/". Any file whose on-disk bytes differ from the
archived bytes was rewritten after extraction -- by linkfix, a relink pass or a
save in the IDE. That gives an exact per-file account without reading any log.

WHERE is a venv folder, a project with a venv, or an install-record folder.
"""

import argparse
import hashlib
import json
import sys
import zipfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from lvpmdata import archive, installed, installed_pairs  # noqa: E402


def md5(path, _buf=1 << 20):
    h = hashlib.md5()
    with path.open("rb") as f:
        while chunk := f.read(_buf):
            h.update(chunk)
    return h.hexdigest()


def cmd_snapshot(a):
    recs = installed(a.where, a.packages or None)
    result, totals = {}, [0, 0]
    for name, rec in recs.items():
        entries = {}
        for f in rec["files"]:
            p = Path(f)
            if p.is_file():
                entries[f] = {"size": p.stat().st_size, "md5": md5(p)}
                totals[0] += 1
            else:
                entries[f] = None
                totals[1] += 1
        result[f"{name}-{rec['version']}"] = {"declared": len(rec["files"]), "files": entries}
    out = {"where": str(a.where), "packages": result}
    Path(a.out).write_text(json.dumps(out, indent=1), encoding="utf-8")
    print(f"{len(recs)} packages, {totals[0]} files present, {totals[1]} missing")
    for stem, d in result.items():
        miss = sum(1 for v in d["files"].values() if v is None)
        print(f"  {stem:<62} {d['declared']:>4} declared" + (f"  {miss} MISSING" if miss else ""))
    print(f"\nwrote {a.out}")


def cmd_diff(a):
    A = json.loads(Path(a.before).read_text(encoding="utf-8"))
    B = json.loads(Path(a.after).read_text(encoding="utf-8"))
    pk = sorted(set(A["packages"]) | set(B["packages"]))
    tot = {"same": 0, "changed": 0, "added": 0, "removed": 0}
    for p in pk:
        fa = A["packages"].get(p, {}).get("files", {})
        fb = B["packages"].get(p, {}).get("files", {})
        rows = []
        for rel in sorted(set(fa) | set(fb)):
            x, y = fa.get(rel), fb.get(rel)
            if x is None and y is None:
                continue
            if x is None:
                rows.append(("added", rel)); tot["added"] += 1
            elif y is None:
                rows.append(("removed", rel)); tot["removed"] += 1
            elif x["md5"] != y["md5"]:
                rows.append(("changed", rel)); tot["changed"] += 1
            else:
                tot["same"] += 1
        if rows:
            print(f"\n=== {p} ===")
            for kind, rel in rows[: a.limit]:
                print(f"  {kind:<8} {rel}")
            if len(rows) > a.limit:
                print(f"  ... {len(rows) - a.limit} more")
    print(f"\nsame={tot['same']}  changed={tot['changed']}  "
          f"added={tot['added']}  removed={tot['removed']}")


def cmd_vs_archive(a):
    grand = [0, 0]
    for name, rec in installed(a.where, a.packages or None).items():
        stem = f"{name}-{rec['version']}"
        vip = archive(stem)
        if vip is None:
            print(f"{stem:<62} (not in lvpm's cache)")
            continue
        same = diff = 0
        with zipfile.ZipFile(vip) as z:
            for rr, member, p in installed_pairs(rec, vip):
                if not p.is_file():
                    continue
                if hashlib.md5(z.read(member)).hexdigest() == md5(p):
                    same += 1
                else:
                    diff += 1
                    if a.list:
                        print(f"    rewritten {rr}")
        grand[0] += same; grand[1] += diff
        pct = (100 * diff / (same + diff)) if (same + diff) else 0
        print(f"{stem:<62} rewritten {diff:>4}/{same + diff:<4} ({pct:5.1f}%)")
    tot = grand[0] + grand[1]
    print(f"\ntotal: {grand[1]}/{tot} files differ from the packaged bytes "
          f"({100 * grand[1] / tot if tot else 0:.1f}%)")


def main():
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)

    s = sub.add_parser("snapshot", help="record the on-disk state of installed packages")
    s.add_argument("where", type=Path)
    s.add_argument("packages", nargs="*")
    s.add_argument("-o", "--out", default="inventory.json")
    s.set_defaults(func=cmd_snapshot)

    d = sub.add_parser("diff", help="compare two snapshots")
    d.add_argument("before")
    d.add_argument("after")
    d.add_argument("--limit", type=int, default=15, help="rows shown per package")
    d.set_defaults(func=cmd_diff)

    v = sub.add_parser("vs-archive", help="compare on-disk bytes to the cached archive's")
    v.add_argument("where", type=Path)
    v.add_argument("packages", nargs="*")
    v.add_argument("--list", action="store_true", help="name every rewritten file")
    v.set_defaults(func=cmd_vs_archive)

    a = ap.parse_args()
    a.func(a)


if __name__ == "__main__":
    main()
