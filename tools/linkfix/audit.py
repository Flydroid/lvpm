#!/usr/bin/env python3
r"""Audit packages for venv-readiness without installing or starting LabVIEW.

Measured 2026-09-17 on LUnit and JSONtext installed into a venv with
`--no-relink`: the VIs load and run in LabVIEW 2026 untouched, no VI asks to be
saved, only the .lvlib's `LVVersion` gets dirty. So the LabVIEW relink pass is
optional for a venv -- provided two things hold per VI, which this script
checks straight from the archives in lvpm's package cache:

  1. Separate compiled code.  A VI carrying a `VICD` block embeds compiled
     code for the version it was saved with; a newer LabVIEW recompiles it in
     memory on every load. Measured cost is a load-time pause, NOT a disk
     write: lava_lib_tree_control_api (25 members, all embedded, saved in 8.6)
     recompiled on open from a venv and nothing was written until a member was
     saved by hand -- and that save rewrote the whole LLB and moved that member
     to 26.3, which is what breaks the version independence LVAddons buys.
  2. Portable linker paths.  Every `PTH0` in the linker tables should be a
     `<vilib>`-style token or relative. A drive-letter absolute is the build
     machine leaking through, and only resolves via LabVIEW's search fallback.

    python tools/linkfix/audit.py oglib_error jki_lib_caraya             # by name
    python tools/linkfix/audit.py --manifest lvpm.toml                   # a project's dependencies
    python tools/linkfix/audit.py --installed C:\Git\venv-check          # what a venv has installed
    python tools/linkfix/audit.py --manifest lvpm.toml --show absolute   # list the paths
    python tools/linkfix/audit.py --manifest lvpm.toml --show missing    # unresolvable tokens

A token path is "missing" when it points neither into the LabVIEW install nor
into any audited package's own files. Those are the real dependency gaps; a
token that another package in the same set provides is fine.
"""

import argparse
import re
import struct
import sys
import zipfile
from collections import Counter, defaultdict
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from deptree import TOKEN_SUBDIR, pth0_records  # noqa: E402
from linker_diff import rsrc_blocks, save_version  # noqa: E402
from lvpmdata import DEFAULT_LV, archive, installed, manifest_packages  # noqa: E402

VI_EXT = {".vi", ".vim", ".vit", ".ctl", ".ctt"}
LINK_BLOCKS = ("LIvi", "LIbd", "LIfp", "LIds")
XML_EXT = {".lvlib", ".lvclass", ".lvproj", ".xctl", ".lvsc"}


def package_names(packages, manifest=None, where=None):
    """'name' / 'name-version' list from the command line, an lvpm.toml's
    [dependencies] and what a venv has installed, in that order."""
    names = list(packages)
    if manifest:
        names += manifest_packages(manifest)
    if where:
        names += [f"{n}-{r['version']}" for n, r in installed(where).items()]
    return names


def llb_members(data):
    """Yield (name, bytes) for every file inside an LLB.

    An LLB is itself an RSRC container (type LVAR) with the member files laid
    out verbatim inside it, each a complete RSRC file with its usual second
    header copy at its own rsrc_info_offset. Scanning for 'RSRC\\r\\n' and
    accepting only offsets whose header copy agrees finds the primaries; the
    member's own `TITL` block names it (a VI does not know its filename, but
    the LLB directory stores it there).
    """
    i = data.find(b"RSRC\r\n", 1)
    while i > 0:
        try:
            info_off, info_sz, data_off, data_sz = struct.unpack_from(">4I", data, i + 16)
        except struct.error:
            break
        end = max(info_off + info_sz, data_off + data_sz)
        copy = i + info_off
        if (data[copy:copy + 6] == b"RSRC\r\n"
                and data[i + 16:i + 32] == data[copy + 16:copy + 32]
                and i + end <= len(data)):
            member = data[i:i + end]
            kind = member[8:12]
            ext = {b"LVIN": ".vi", b"LVCC": ".ctl"}.get(kind, ".bin")
            name = None
            blocks = rsrc_blocks(member)
            if blocks and blocks.get("TITL"):
                t = blocks["TITL"][0]
                name = t[1:1 + t[0]].decode("latin-1", errors="replace")
            yield (name or f"member@{i}") + ext, member
            i = data.find(b"RSRC\r\n", i + end)
        else:
            i = data.find(b"RSRC\r\n", i + 1)


def archive_files(vip):
    """{logical path under the target dir: bytes} for a .vip.

    Members are 'File Group N/<rel>'; the group's Target Dir is in the spec,
    but for vi.lib/menus/etc. packages the rel part already starts with the
    subdirectory name, which is all the token matching needs. An .llb is
    kept as itself AND expanded into 'x.llb/member.vi' entries, since linker
    paths address LLB members as if the LLB were a folder.
    """
    out = {}
    with zipfile.ZipFile(vip) as z:
        for n in z.namelist():
            if n.startswith("File Group") and "/" in n and not n.endswith("/"):
                rel = n.split("/", 1)[1]
                data = z.read(n)
                out[rel] = data
                if rel.lower().endswith(".llb") and data[:4] == b"RSRC":
                    for name, member in llb_members(data):
                        out[rel + "/" + name] = member
    return out


def classify(path, lv, provided):
    """One of token-ok / token-missing / absolute / relative."""
    head, _, rest = path.partition("/")
    if head in TOKEN_SUBDIR:
        sub = TOKEN_SUBDIR[head]
        key = (sub + "/" + rest).lower()
        if any(key == p or key.startswith(p + "/") or p.startswith(key + "/")
               for p in provided):
            return "token-ok"
        real = Path(lv) / sub / rest
        for c in [real, *real.parents]:
            if len(c.parts) <= len(Path(lv).parts):
                break
            if c.exists():
                return "token-ok"
        return "token-missing"
    if len(head) == 1 and head.isalpha():
        return "absolute"
    return "relative"


def audit(vip, lv, provided):
    files = archive_files(vip)
    r = dict(vis=0, embedded=0, versions=Counter(), kinds=Counter(),
             examples=defaultdict(set), xml_versions=Counter())
    for rel, data in files.items():
        suf = Path(rel).suffix.lower()
        if suf in XML_EXT:
            m = re.search(rb'LVVersion="(\d+)"', data)
            if m:
                r["xml_versions"][m.group(1)[:2].decode()] += 1
            continue
        if suf not in VI_EXT:
            continue
        blocks = rsrc_blocks(data)
        if blocks is None:
            continue
        r["vis"] += 1
        if "VICD" in blocks:
            r["embedded"] += 1
        r["versions"][save_version(blocks) or "?"] += 1
        paths = set()
        for ident in LINK_BLOCKS:
            for sec in blocks.get(ident, []):
                for comps in pth0_records(sec):
                    paths.add("/".join(comps).lower())
        for p in paths:
            k = classify(p, lv, provided)
            r["kinds"][k] += 1
            if k != "token-ok":
                r["examples"][k].add(p)
    return r


def main():
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("packages", nargs="*", help="name or name-version")
    ap.add_argument("--manifest", type=Path, help="audit every dependency an lvpm.toml lists")
    ap.add_argument("--installed", type=Path, metavar="WHERE",
                    help="audit every package installed in a venv (or a project with one)")
    ap.add_argument("--labview", type=Path, default=DEFAULT_LV)
    ap.add_argument("--show", choices=["absolute", "missing", "relative"],
                    help="print the offending paths of this kind per package")
    a = ap.parse_args()

    names = package_names(a.packages, a.manifest, a.installed)
    if not names:
        ap.error("give package names, --manifest or --installed")

    vips, uncached = {}, []
    for n in names:
        v = archive(n)
        (vips.__setitem__(n, v) if v else uncached.append(n))

    # What the whole set provides, so cross-package tokens count as resolved.
    provided = set()
    for v in vips.values():
        provided |= {rel.lower() for rel in archive_files(v)}

    rows, totals = [], Counter()
    for n, v in vips.items():
        r = audit(v, a.labview, provided)
        rows.append((n, r))
        totals["vis"] += r["vis"]
        totals["embedded"] += r["embedded"]
        totals.update(r["kinds"])

    w = max(len(n) for n in vips) if vips else 10
    print(f"{'package':{w}}  {'VIs':>4} {'embed':>5} {'saved':>7}  "
          f"{'token':>5} {'miss':>4} {'abs':>4} {'rel':>4}")
    for n, r in rows:
        ver = ",".join(sorted(r["versions"])) if r["versions"] else "-"
        k = r["kinds"]
        flag = ""
        if r["embedded"]:
            flag += " E"
        if k["absolute"]:
            flag += " A"
        if k["token-missing"]:
            flag += " M"
        print(f"{n:{w}}  {r['vis']:>4} {r['embedded']:>5} {ver:>7}  "
              f"{k['token-ok']:>5} {k['token-missing']:>4} {k['absolute']:>4} "
              f"{k['relative']:>4} {flag}")
        if a.show:
            key = {"absolute": "absolute", "missing": "token-missing",
                   "relative": "relative"}[a.show]
            for p in sorted(r["examples"][key])[:40]:
                print(f"      {p}")
    print(f"\n{'total':{w}}  {totals['vis']:>4} {totals['embedded']:>5} {'':>7}  "
          f"{totals['token-ok']:>5} {totals['token-missing']:>4} "
          f"{totals['absolute']:>4} {totals['relative']:>4}")
    print("\nflags: E = VIs with embedded compiled code (no separate compiled code)"
          "\n       A = drive-letter absolute linker paths (build machine leaked)"
          "\n       M = token paths that neither LabVIEW nor this package set provides")
    if uncached:
        print(f"\nnot in lvpm's cache ({len(uncached)}): " + ", ".join(uncached))


if __name__ == "__main__":
    main()
