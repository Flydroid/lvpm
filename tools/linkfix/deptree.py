#!/usr/bin/env python3
r"""Read a VI's declared dependencies out of its linker blocks, and audit them.

Relinking rewrites the linker tables (`LIvi`, `LIbd`, `LIfp`, `LIds`), but those
tables are NOT byte-reproducible: measured on astemes_lib_lunit, repeated relink
passes leave ~24 of 230 files with a different `LIfp` of identical length —
LabVIEW serialises the same entries in a different order each time. Comparing
files by hash therefore never converges and says nothing useful.

What each table really holds is a SET of paths, and that set IS stable: across
four consecutive relink passes the byte diff moved 12-19 files per pass while the
dependency sets moved zero. So compare dependency sets, not bytes.

    python tools/linkfix/deptree.py show "<vi>"                     # one VI's deps
    python tools/linkfix/deptree.py check <tree>                    # do all deps resolve?
    python tools/linkfix/deptree.py compare <tree-a> <tree-b>       # set-level diff

`check` is the correctness gate, and it needs no reference install: a declared
path that does not resolve on this machine is exactly what makes LabVIEW open its
"searching for missing subVI" dialog. Measured on LUnit 2.0.13.6 --- packaged
19 unresolvable of 188 declared.
Paths are `PTH0` records, the layout documented in docs/vi-server-protocol.md:

    "PTH0" | u32 byte length | u32 component count | pascal components...

with the length field covering the count field plus the components. Absolute
paths carry the drive letter as the first component; `<vilib>`-style tokens
appear verbatim as the first component and are expanded here against --labview.
"""

import argparse
import os
import struct
import sys
from collections import Counter, defaultdict
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from lvpmdata import DEFAULT_LV  # noqa: E402
from linker_diff import rsrc_blocks  # noqa: E402

VI_EXT = {".vi", ".vim", ".vit", ".ctl", ".ctt"}

# Target-directory tokens as they appear as a PTH0 first component.
TOKEN_SUBDIR = {
    "<vilib>": "vi.lib",
    "<userlib>": "user.lib",
    "<instrlib>": "instr.lib",
    "<resource>": "resource",
    "<menus>": "menus",
    "<help>": "help",
    "<examples>": "examples",
    "<project>": "project",
}

MAX_COMPONENTS = 128   # a sane ceiling; guards against matching stray "PTH0"


def pth0_records(buf):
    """Every plausible PTH0 record in buf, as tuples of path components."""
    out = []
    i = 0
    while True:
        i = buf.find(b"PTH0", i)
        if i < 0:
            return out
        try:
            # u32 byte length, then u16 path type (0 absolute/token, 1
            # relative), then u16 component count. Reading type+count as one
            # u32 made every relative path look like 65538 components and
            # dropped it — which is why audit.py reported zero relative paths.
            length = struct.unpack_from(">I", buf, i + 4)[0]
            _ptype, count = struct.unpack_from(">HH", buf, i + 8)
        except struct.error:
            i += 4
            continue
        if length < 4 or i + 8 + length > len(buf) or count > MAX_COMPONENTS:
            i += 4
            continue
        comps, p, end, ok = [], i + 12, i + 8 + length, True
        for _ in range(count):
            if p >= end:
                ok = False
                break
            n = buf[p]
            if p + 1 + n > end:
                ok = False
                break
            # spec files and paths are latin-1-ish; lossless for a byte round trip
            comps.append(buf[p + 1:p + 1 + n].decode("latin-1"))
            p += 1 + n
        if ok and comps:
            out.append(tuple(comps))
        i += 4


def deps_by_block(path):
    """{block ident: {path string}} for one RSRC file. Empty if not RSRC."""
    blocks = rsrc_blocks(Path(path).read_bytes())
    if blocks is None:
        return {}
    out = {}
    for k, sections in blocks.items():
        ident = k.decode(errors="replace") if isinstance(k, bytes) else str(k)
        if not ident.startswith("LI"):
            continue
        paths = set()
        for sec in sections:
            for comps in pth0_records(sec):
                paths.add("/".join(comps).lower())
        if paths:
            out[ident] = paths
    return out


def tokenise(dep, lv):
    """Rewrite an absolute path into the installation into its `<vilib>`-style token.

    A relink through VI Server
    writes `<vilib>/x` for the same target. Both resolve; only the token form
    survives the installation moving. Comparisons must not treat them as
    different dependencies -- doing so overstated the gap by 6 paths in 45.
    """
    root = str(lv).replace("\\", "/").lower().rstrip("/")
    drive, _, rest = root.partition(":")
    bare = f"{drive}{rest}/"          # PTH0 renders "C:" as a bare "c" component
    if not dep.startswith(bare):
        return dep
    tail = dep[len(bare):]
    for token, sub in TOKEN_SUBDIR.items():
        if tail.startswith(sub + "/"):
            return token + "/" + tail[len(sub) + 1:]
    return dep


def deps(path, lv=None):
    """The union of every linker table's paths — the stable comparison unit."""
    s = set()
    for v in deps_by_block(path).values():
        s |= v
    return {tokenise(d, lv) for d in s} if lv else s


def rsrc_files(root):
    root = Path(root)
    for p in sorted(root.rglob("*")):
        if p.is_file() and p.suffix.lower() in VI_EXT:
            yield p.relative_to(root), p


def resolve(dep, vi_dir, lv):
    """Turn a declared path into a filesystem path to test for existence."""
    head = dep.split("/", 1)[0]
    rest = dep.split("/", 1)[1] if "/" in dep else ""
    if head in TOKEN_SUBDIR:
        return Path(lv) / TOKEN_SUBDIR[head] / rest.replace("/", os.sep)
    # An absolute path contributes its drive letter as the first component.
    if len(head) == 1 and head.isalpha():
        return Path(head + ":" + os.sep + rest.replace("/", os.sep))
    return Path(vi_dir) / dep.replace("/", os.sep)


def resolvable(p):
    """True if p exists, or lives inside a container (.llb/.lvclass) that does.

    Members of an LLB or a class are addressed as if the container were a
    directory, so the file itself never exists on disk under that name.
    """
    p = Path(p)
    for cand in [p, *p.parents]:
        if len(cand.parts) <= 1:
            break
        if cand.exists():
            return True
    return False


def cmd_show(a):
    for target in a.paths:
        print(f"\n=== {target} ===")
        by_block = deps_by_block(target)
        if not by_block:
            print("   no linker tables (not RSRC, or nothing declared)")
            continue
        for ident in sorted(by_block):
            print(f"   {ident}  ({len(by_block[ident])} paths)")
            for d in sorted(by_block[ident]):
                print(f"      {d}")
    return 0


def cmd_check(a):
    total = ok = 0
    bad = defaultdict(list)
    roots = Counter()
    for rel, p in rsrc_files(a.tree):
        for d in sorted(deps(p)):
            total += 1
            if resolvable(resolve(d, p.parent, a.labview)):
                ok += 1
            else:
                bad[str(rel)].append(d)
                roots[d.split("/", 1)[0]] += 1

    print(f"{a.tree}")
    print(f"  declared {total}   resolvable {ok}   UNRESOLVABLE {total - ok}")
    if not bad:
        print("  every declared dependency resolves - no load-time search needed")
        return 0
    print(f"  files affected: {len(bad)}   unresolvable roots: {dict(roots)}")
    for rel in sorted(bad)[:a.limit]:
        print(f"\n  {rel}")
        for d in bad[rel][:8]:
            print(f"      {d}")
    if len(bad) > a.limit:
        print(f"\n  ... and {len(bad) - a.limit} more files (raise --limit)")
    return 1


def cmd_compare(a):
    A, B = Path(a.tree_a), Path(a.tree_b)
    equal = subset = superset = other = missing_file = 0
    only_b_total = only_a_total = 0
    worst = []
    for rel, pa in rsrc_files(A):
        pb = B / rel
        if not pb.is_file():
            missing_file += 1
            continue
        da, db = deps(pa, a.labview), deps(pb, a.labview)
        only_b, only_a = db - da, da - db
        only_b_total += len(only_b)
        only_a_total += len(only_a)
        if da == db:
            equal += 1
        elif da < db:
            subset += 1
        elif da > db:
            superset += 1
        else:
            other += 1
        if only_a or only_b:
            worst.append((len(only_b), len(only_a), str(rel), sorted(only_b), sorted(only_a)))

    print(f"A = {A}\nB = {B}\n")
    print(f"  identical dependency sets : {equal}")
    print(f"  A subset of B (A lacks)   : {subset}")
    print(f"  A superset of B (A adds)  : {superset}")
    print(f"  neither (both directions) : {other}")
    if missing_file:
        print(f"  absent from B             : {missing_file}")
    print(f"\n  paths B has that A lacks  : {only_b_total}")
    print(f"  paths A has that B lacks  : {only_a_total}")

    ext = Counter()
    for nb, na, rel, ob, oa in worst:
        for d in ob:
            ext[Path(d).suffix or "(none)"] += 1
    if ext:
        print(f"  B-only paths by extension : {dict(ext.most_common())}")

    worst.sort(reverse=True)
    for nb, na, rel, ob, oa in worst[:a.limit]:
        print(f"\n  {rel}   (B-only {nb}, A-only {na})")
        for d in ob[:5]:
            print(f"      B only: {d}")
        for d in oa[:5]:
            print(f"      A only: {d}")
    return 0


def main():
    # Shared options live on a parent so they are accepted either side of the
    # subcommand: `deptree.py --limit 3 check X` and `deptree.py check X --limit 3`.
    common = argparse.ArgumentParser(add_help=False)
    common.add_argument("--labview", type=Path, default=DEFAULT_LV,
                        help="installation used to expand <vilib> and friends")
    common.add_argument("--limit", type=int, default=10,
                        help="how many offending files to list (default 10)")

    ap = argparse.ArgumentParser(
        parents=[common], description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)

    s = sub.add_parser("show", parents=[common],
                       help="print one or more VIs' declared dependencies")
    s.add_argument("paths", nargs="+")
    s.set_defaults(func=cmd_show)

    s = sub.add_parser("check", parents=[common],
                       help="do all declared dependencies resolve?")
    s.add_argument("tree")
    s.set_defaults(func=cmd_check)

    s = sub.add_parser("compare", parents=[common],
                       help="set-level dependency diff between two trees")
    s.add_argument("tree_a")
    s.add_argument("tree_b")
    s.set_defaults(func=cmd_compare)

    a = ap.parse_args()
    return a.func(a)


if __name__ == "__main__":
    sys.exit(main())
