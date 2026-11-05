#!/usr/bin/env python3
r"""Split a VI's link sections into objects the way src/linkfix.rs does, and
compare packaged vs IDE-saved copies object by object.

pylabview's per-kind layouts are incomplete for LabVIEW 2020 class-API links
(`VIPI` & co.: it stops after the first object), so this uses the same
generic walk as the Rust: an object is `2 | ident | pad4 | qualname | pad2 |
PTH0 | flag | tail`, and the tail runs to the next `2 | ident` whose basic
part parses, or the closing `3`.

    python tools/linkfix/linkobjs.py show <vi>
    python tools/linkfix/linkobjs.py shift <packaged.vi> <saved.vi>   # do tails change when they move?
    python tools/linkfix/linkobjs.py survey <venv-check .project> <venv-novaxc .project> <oracle.txt>
"""

import struct
import sys
from collections import Counter, defaultdict
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from linker_diff import rsrc_blocks  # noqa: E402


def version(blocks):
    v = struct.unpack_from(">I", blocks["LVSR"][0], 0)[0]
    return (((v >> 28) & 0xF) * 10 + ((v >> 24) & 0xF), (v >> 20) & 0xF)


def pad(pos, n):
    return (n - pos % n) % n


def parse_pth(b, pos):
    if b[pos:pos + 4] != b"PTH0":
        raise ValueError("no PTH0")
    totlen, tp, count = struct.unpack_from(">IHH", b, pos + 4)
    if count > 128:
        raise ValueError("count")
    p = pos + 12
    comps = []
    for _ in range(count):
        n = b[p]
        comps.append(b[p + 1:p + 1 + n])
        p += 1 + n
    return (tp, comps), p


def parse_basic(b, pos, ver):
    ident = b[pos:pos + 4]
    if len(ident) < 4 or not all(chr(c).isalnum() for c in ident):
        raise ValueError("ident")
    pos += 4
    pos += pad(pos, 4)
    n = struct.unpack_from(">I", b, pos)[0]
    if n > 64:
        raise ValueError("qual")
    pos += 4
    qual = []
    for _ in range(n):
        l = b[pos]
        qual.append(b[pos + 1:pos + 1 + l])
        pos += 1 + l
    pos += pad(pos, 2)
    path, pos = parse_pth(b, pos)
    flag = struct.unpack_from(">I", b, pos)[0]
    pos += 4
    return ident, qual, path, flag, pos


def objects(sec, ver):
    """[(ident, qual, path, flag, tail_bytes, obj_start, tail_start)]"""
    assert struct.unpack_from(">H", sec, 0)[0] == 1
    pos = 6
    if ver < (14, 0):
        l = sec[pos]
        pos += 1 + l + (l + 1) % 2
        words = struct.unpack_from(">H", sec, pos)[0]
        pos += 2 + 2 * words
    count = struct.unpack_from(">I", sec, pos)[0]
    pos += 4
    out = []
    while True:
        m = struct.unpack_from(">H", sec, pos)[0]
        if m == 3:
            break
        assert m == 2, m
        start = pos
        ident, qual, path, flag, after = parse_basic(sec, pos + 2, ver)
        q = after
        while True:
            mm = struct.unpack_from(">H", sec, q)[0]
            if mm == 3 and q + 2 == len(sec):
                break
            if mm == 2:
                try:
                    parse_basic(sec, q + 2, ver)
                    break
                except Exception:
                    pass
            q += 1
        out.append((ident, qual, path, flag, sec[after:q], start, after))
        pos = q
    assert len(out) == count, (len(out), count)
    return out


def li_objects(data):
    blocks = rsrc_blocks(data)
    ver = version(blocks)
    res = {}
    for ident in ("LIvi", "LIbd", "LIfp", "LIds"):
        for sec in blocks.get(ident, []):
            res[ident] = objects(sec, ver)
    return ver, res


def cmd_show(a):
    ver, res = li_objects(Path(a[0]).read_bytes())
    print("version", ver)
    for ident, objs in res.items():
        print(f"--- {ident}: {len(objs)} objects")
        for id_, qual, (tp, comps), flag, tail, start, tstart in objs:
            print(f"  {id_.decode()} @{start} qual={[q.decode('latin-1') for q in qual]} tp={tp} {'/'.join(c.decode('latin-1') for c in comps)} flag={flag} tail={len(tail)}B")


def shift_report(pkg, sav, tally, examples, key=""):
    """For objects present (same ident+qual last) in both, did the tail bytes
    change, and did the tail's section position shift by a non-multiple of 4?"""
    v1, r1 = li_objects(pkg)
    v2, r2 = li_objects(sav)
    for ident in r1:
        if ident not in r2:
            continue
        o2 = {(o[0], o[1][-1] if o[1] else b""): o for o in r2[ident]}
        for o in r1[ident]:
            k = (o[0], o[1][-1] if o[1] else b"")
            if k not in o2:
                continue
            p = o2[k]
            shift = (p[6] - o[6]) % 4
            same = o[4] == p[4]
            tally[(o[0].decode(), "shift%4=" + str(shift), "tail same" if same else "tail DIFFERS")] += 1
            if not same and len(examples[o[0]]) < 2:
                examples[o[0]].append((key, shift, o[4][:48].hex(" "), p[4][:48].hex(" ")))


def cmd_shift(a):
    tally, ex = Counter(), defaultdict(list)
    shift_report(Path(a[0]).read_bytes(), Path(a[1]).read_bytes(), tally, ex)
    for k, v in sorted(tally.items()):
        print(v, k)


def cmd_survey(a):
    check, ide, oracle = Path(a[0]), Path(a[1]), Path(a[2])
    tally, ex = Counter(), defaultdict(list)
    n = 0
    for rel in oracle.read_text(encoding="utf-8").splitlines():
        rel = rel.strip()
        if not rel:
            continue
        try:
            shift_report((check / rel).read_bytes(), (ide / rel).read_bytes(), tally, ex, rel)
            n += 1
        except Exception as e:  # noqa: BLE001
            print("skip", rel, "->", e)
    print(f"\n{n} packaged/IDE-saved pairs compared. Per kind: does a tail change when it moves?")
    for k, v in sorted(tally.items()):
        print(f"  {v:>4}  {k}")
    print("\nexamples of differing tails (kind, shift, packaged..., saved...):")
    for kind, lst in ex.items():
        for key, shift, a_, b_ in lst:
            print(f"  {kind.decode()} shift {shift} {key}\n     pkg   {a_}\n     saved {b_}")


def tail_paths(tail):
    """Every PTH0 inside a tail (the VILSPathRef, parent-class links), as strings."""
    out, i = [], 0
    while (i := tail.find(b"PTH0", i)) >= 0:
        try:
            (tp, comps), _ = parse_pth(tail, i)
            out.append((tp, b"/".join(comps).decode("latin-1")))
        except Exception:
            pass
        i += 4
    return out


def semantic(objs):
    return [
        (id_.decode(), [q.decode("latin-1") for q in qual], tp, "/".join(c.decode("latin-1") for c in comps), tail_paths(tail))
        for id_, qual, (tp, comps), flag, tail, start, tstart in objs
    ]


def cmd_oracle(a):
    """Compare the fixer's output with what the IDE saved, link by link."""
    fixed, ide, oracle = Path(a[0]), Path(a[1]), Path(a[2])
    files = same = 0
    diffs = Counter()
    shown = 0
    for rel in oracle.read_text(encoding="utf-8").splitlines():
        rel = rel.strip()
        if not rel:
            continue
        files += 1
        try:
            _, rf = li_objects((fixed / rel).read_bytes())
            _, ri = li_objects((ide / rel).read_bytes())
        except Exception as e:  # noqa: BLE001
            diffs["unparseable: " + str(e)[:40]] += 1
            continue
        ok = True
        for ident in sorted(set(rf) | set(ri)):
            sf, si = semantic(rf.get(ident, [])), semantic(ri.get(ident, []))
            # Match objects by kind and target file: the IDE (LabVIEW 26) may
            # add or drop objects a 2020 file never had; those are reported but
            # are not a fixer error.
            key = lambda o: (o[0], o[3].lower().rsplit("/", 1)[-1])  # noqa: E731
            di = {key(i): i for i in si}
            for f in sf:
                i = di.get(key(f))
                if i is None:
                    diffs[f"{ident} {f[0]}: only in fixed (info)"] += 1
                    continue
                if f[1] != i[1]:
                    diffs[f"{ident} {f[0]}: qualified name"] += 1
                    ok = False
                    if shown < 12:
                        print(f"  QUAL {rel}\n     fixed {f[1]}\n     ide   {i[1]}")
                        shown += 1
                if (f[2], f[3].lower()) != (i[2], i[3].lower()):
                    diffs[f"{ident} {f[0]}: path"] += 1
                    ok = False
                    if shown < 12:
                        print(f"  PATH {rel} {f[0]}\n     fixed {f[2]} {f[3]}\n     ide   {i[2]} {i[3]}")
                        shown += 1
                if [(t, p.lower()) for t, p in f[4]] != [(t, p.lower()) for t, p in i[4]]:
                    diffs[f"{ident} {f[0]}: tail paths"] += 1
                    ok = False
                    if shown < 12:
                        print(f"  TAIL {rel} {f[0]}\n     fixed {f[4]}\n     ide   {i[4]}")
                        shown += 1
        same += ok
    print(f"\n{files} oracle files: {same} match the IDE's link tables exactly, {files - same} differ")
    for k, v in diffs.most_common():
        print(f"  {v:>4}  {k}")


if __name__ == "__main__":
    cmd = sys.argv[1]
    {"show": cmd_show, "shift": cmd_shift, "survey": cmd_survey, "oracle": cmd_oracle}[cmd](sys.argv[2:])
