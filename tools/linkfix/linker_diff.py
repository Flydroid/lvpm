#!/usr/bin/env python3
r"""Compare LabVIEW resource blocks between a package's archived VIs and disk.

A `.vi` is a RSRC container of named blocks. Relinking (linkfix, a load/save
pass, or LabVIEW's Mass Compile) rewrites some of those blocks — the linker tables
(`LIvi`, `LIbd`, `LIfp`, `LIds`), the save-version record (`vers`, `LVSR`), the
compiled code (`VICD`) — while payload-only installs leave every byte as it was
packaged. Diffing block-by-block shows exactly what "relinked" means on disk.

    python tools/linkfix/linker_diff.py C:\Git\venv-check jki_lib_serialization        # summary
    python tools/linkfix/linker_diff.py C:\Git\venv-check jdp_science_jsontext --strings "some.vi"

The packaged bytes come from lvpm's cached archive and the installed files from
lvpm's install record (see lvpmdata.py); WHERE is a venv or a project with one.

The RSRC layout is the one pylabview documents (LVrsrcontainer/LVblock):
header at 0 and a second copy at rsrc_info_offset; after the copy a
BlockInfoListHeader whose blockinfo_offset (relative to rsrc_info_offset)
locates a count-minus-one array of BlockHeaders; each header's offset (same
base) locates count-minus-one BlockSectionStarts; each section's data_offset
(relative to rsrc_data_offset) locates a u32 size + raw bytes.
"""

import argparse
import struct
import sys
import zipfile
from collections import Counter, defaultdict
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from lvpmdata import archive, installed, installed_pairs  # noqa: E402

VI_EXT = {".vi", ".vim", ".vit", ".ctl", ".ctt"}


def rsrc_blocks(data):
    """{ident: [section bytes, ...]} for one RSRC file, or None if not RSRC."""
    if len(data) < 32 or data[:4] != b"RSRC":
        return None
    info_off, _info_sz, data_off, _data_sz = struct.unpack_from(">4I", data, 16)
    # After the second header copy: BlockInfoListHeader (20 bytes).
    bilh = info_off + 32
    blockinfo_off = struct.unpack_from(">I", data, bilh + 12)[0]
    base = info_off + blockinfo_off
    count = struct.unpack_from(">I", data, base)[0] + 1
    out = {}
    for i in range(count):
        hdr = base + 4 + 12 * i
        ident = data[hdr:hdr + 4].decode("latin-1")
        scount = struct.unpack_from(">I", data, hdr + 4)[0] + 1
        soff = struct.unpack_from(">I", data, hdr + 8)[0]
        sections = []
        for j in range(scount):
            st = base + soff + 20 * j
            d_off = struct.unpack_from(">I", data, st + 12)[0]
            pos = data_off + d_off
            size = struct.unpack_from(">I", data, pos)[0]
            sections.append(bytes(data[pos + 4:pos + 4 + size]))
        out[ident] = sections
    return out


def save_version(blocks):
    """Decode the version string from the `vers` block ('26.3', '20.0', ...)."""
    secs = blocks.get("vers")
    if not secs or len(secs[0]) < 7:
        return None
    b = secs[0]
    n = b[6]
    try:
        return b[7:7 + n].decode("latin-1")
    except Exception:
        return None


def strings_of(b, minlen=4):
    out, cur = set(), []
    for ch in b:
        if 0x20 <= ch < 0x7F:
            cur.append(ch)
        else:
            if len(cur) >= minlen:
                out.add(bytes(cur).decode("latin-1"))
            cur = []
    if len(cur) >= minlen:
        out.add(bytes(cur).decode("latin-1"))
    return out


def pairs_for(where, package):
    """Yield (rel, packaged bytes, disk bytes) for every VI-ish file present in
    both lvpm's cached archive and on disk."""
    rec = installed(where, [package])[package]
    vip = archive(f"{package}-{rec['version']}")
    if vip is None:
        raise SystemExit(f"{package}-{rec['version']} is not in lvpm's cache (`lvpm cache add <file>`)")
    with zipfile.ZipFile(vip) as z:
        for rel, member, p in installed_pairs(rec, vip):
            if Path(rel).suffix.lower() not in VI_EXT or not p.is_file():
                continue
            yield rel, z.read(member), p.read_bytes()


def main():
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("where", type=Path, help="a venv folder or a project with one")
    ap.add_argument("package")
    ap.add_argument("--strings", metavar="SUBSTR",
                    help="for files matching SUBSTR, print the string-level "
                         "diff of each changed block")
    a = ap.parse_args()

    ident_diff = Counter()   # block ident -> how many files it changed in
    ident_only_pkg = Counter()
    ident_only_disk = Counter()
    ver_moves = Counter()    # (packaged ver, disk ver) -> files
    identical = changed = unparsed = 0
    size_delta = defaultdict(int)

    for rel, pkg_bytes, disk_bytes in pairs_for(a.where, a.package):
        if pkg_bytes == disk_bytes:
            identical += 1
            continue
        bp, bd = rsrc_blocks(pkg_bytes), rsrc_blocks(disk_bytes)
        if bp is None or bd is None:
            unparsed += 1
            continue
        changed += 1
        ver_moves[(save_version(bp) or "?", save_version(bd) or "?")] += 1
        want_strings = a.strings and a.strings.lower() in rel.lower()
        if want_strings:
            print(f"\n=== {rel} ===")
        for ident in sorted(set(bp) | set(bd)):
            if ident not in bp:
                ident_only_disk[ident] += 1
                continue
            if ident not in bd:
                ident_only_pkg[ident] += 1
                continue
            if bp[ident] != bd[ident]:
                ident_diff[ident] += 1
                size_delta[ident] += sum(map(len, bd[ident])) - sum(map(len, bp[ident]))
                if want_strings:
                    sp_, sd_ = set(), set()
                    for s in bp[ident]:
                        sp_ |= strings_of(s)
                    for s in bd[ident]:
                        sd_ |= strings_of(s)
                    print(f"  [{ident}]")
                    for s in sorted(sd_ - sp_):
                        print(f"    + {s}")
                    for s in sorted(sp_ - sd_):
                        print(f"    - {s}")

    total = identical + changed + unparsed
    print(f"\n{a.package}: {total} VI-type files compared")
    print(f"  byte-identical to package : {identical}")
    print(f"  rewritten on disk         : {changed}")
    if unparsed:
        print(f"  not RSRC / unparsed       : {unparsed}")

    if ver_moves:
        print("\nsave version, packaged -> disk:")
        for (vp, vd), n in ver_moves.most_common():
            print(f"  {vp:>6} -> {vd:<6} {n:>4} files")

    if ident_diff:
        print(f"\nblocks rewritten (of {changed} changed files):")
        for ident, n in ident_diff.most_common():
            delta = size_delta[ident]
            print(f"  {ident:<6} {n:>4} files   net size {delta:+d} B")
    for label, ctr in (("only in package", ident_only_pkg), ("only on disk", ident_only_disk)):
        if ctr:
            print(f"blocks {label}:")
            for ident, n in ctr.most_common():
                print(f"  {ident:<6} {n:>4} files")


if __name__ == "__main__":
    main()
