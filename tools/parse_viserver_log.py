#!/usr/bin/env python3
"""Summarise a LabVIEW VI Server log into the sequence of calls a client made.

LabVIEW can log every VI Server operation it services. Enable it at runtime by
writing the Application properties `Server:Log File Path` then
`Server:Logging Enabled` (in that order). The setting is runtime-only -- there
is no LabVIEW.ini token and no checkbox in Tools>>Options>>VI Server -- so it
must be re-set every time LabVIEW starts.

    python parse_viserver_log.py vi_server.log
    python parse_viserver_log.py vi_server.log --from 23:38:00 --to 23:45:00
    python parse_viserver_log.py vi_server.log --gap 45 --conn 56C00002

--gap N   split the log wherever it goes quiet for N+ seconds, which separates
          an install burst from a desktop client's background polling without
          having to know the exact start and end times.
--conn    restrict to a single connection cookie.

Opcodes and method IDs outside the known baseline are flagged NEW so unfamiliar
traffic stands out rather than blending in.

Caveat worth remembering when reading the output: LabVIEW logs only messages it
*receives*. The kTS*Return replies are never recorded, so the err=0 reported on
every request says nothing about whether the operation succeeded.
"""

import argparse
import re
from collections import Counter, OrderedDict, defaultdict
from datetime import datetime
from pathlib import Path

# Observed vocabulary across an install (LUnit) and a 9-package
# install (RocketPlot + dependencies).
BASELINE_OPS = {
    0: "kTSClientSaysHeaveno", 1: "kTSAppAttrVectorSend", 2: "kTSVIAttrVectorSend",
    3: "kTSGetVIRefSend", 4: "kTSCallSend", 6: "kTSVIDoMethodSend",
    7: "kTSReleaseRef", 8: "kTSClientBye", 22: "kTSObjAttrVectorSend",
    32: "kTSPingSend",
}
BASELINE_METHS = {
    1003: "Run VI", 1051: "Set Control Value", 1052: "Get Control Value",
    1053: "Get All Control Values",
}

# Application/project-level methods.
WATCH_FOR = {"kTSAppDoMethodSend", "kTSProjMethSend", "kTSLVTargetMethSend",
             "kTSCreateNewVISend", "kTSObjDoMethodSend"}

TS = r"(\d{4}\.\d{2}\.\d{2} - \d{2}:\d{2}:\d{2})"
RE_HDR = re.compile(
    TS + r";\s+## Receiving message\s*\n\s*hdr=\{err=\s*(-?\d+); uID=0x([0-9A-F]+); "
    r"len=\s*(\d+); msg=\"(\w+)\"\((\d+)\)\}\s*\n\s*conn @0x([0-9A-F]+); cookie=0x([0-9A-F]+)")
RE_REF = re.compile(
    TS + r";\s+## kTSGetVIRefSend\s*\n\s*err=(-?\d+); viRef=0x([0-9A-F]+);\s*\n\s*path=\"([^\"]+)\"")
RE_CALL = re.compile(
    TS + r";\s+## Starting VI call\s*\n\s*viRef=0x([0-9A-F]+); ds=0x[0-9A-F]+: \"([^\"]+)\"")
RE_METH = re.compile(
    TS + r";\s+## kTSVIDoMethodSend\s*\n\s*viRef=0x([0-9A-F]+); meth=(\d+) "
    r"\(nihelplauncher://docs/csh\?context=(\w+)\)")

ABBREV = [
    (re.compile(r"C:\\Program Files\\National Instruments\\LabVIEW \d{4}\\", re.I), "<LV>\\"),
    (re.compile(r"C:\\Program Files( \(x86\))?\\National Instruments\\Shared\\LabVIEW CLI\\", re.I), "<LVCLI>\\"),
    (re.compile(r"C:\\Users\\[^\\]+\\AppData\\Local\\Temp\\", re.I), "<TEMP>\\"),
]


def short(p):
    for pat, rep in ABBREV:
        # lambda replacement: the substitutions end in a backslash, which re.sub
        # would otherwise try to interpret as an escape sequence.
        p = pat.sub(lambda _m, r=rep: r, p)
    return p


def ts(s):
    return datetime.strptime(s, "%Y.%m.%d - %H:%M:%S")


def at(day, s):
    return datetime.strptime(f"{day:%Y.%m.%d} - {s}", "%Y.%m.%d - %H:%M:%S")


def main():
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("log", nargs="?", default="vi_server.log")
    ap.add_argument("--from", dest="start", metavar="HH:MM:SS")
    ap.add_argument("--to", dest="end", metavar="HH:MM:SS")
    ap.add_argument("--gap", type=int, default=30,
                    help="idle seconds that starts a new activity segment (default 30)")
    ap.add_argument("--conn", help="restrict to one connection cookie (hex, no 0x)")
    a = ap.parse_args()

    src = Path(a.log)
    text = src.read_text(encoding="utf-8", errors="replace")

    msgs = [{"t": ts(m[0]), "err": int(m[1]), "uid": m[2], "len": int(m[3]),
             "name": m[4], "op": int(m[5]), "conn": m[6], "cookie": m[7]}
            for m in RE_HDR.findall(text)]
    if not msgs:
        print("no messages parsed - is this a VI Server log?")
        return
    refs = [{"t": ts(m[0]), "err": int(m[1]), "ref": m[2], "path": m[3]}
            for m in RE_REF.findall(text)]
    calls = [{"t": ts(m[0]), "ref": m[1], "name": m[2]} for m in RE_CALL.findall(text)]
    meths = [{"t": ts(m[0]), "ref": m[1], "id": int(m[2]), "name": m[3]}
             for m in RE_METH.findall(text)]

    day = msgs[0]["t"]
    lo = at(day, a.start) if a.start else None
    hi = at(day, a.end) if a.end else None

    def keep(r):
        if lo and r["t"] < lo:
            return False
        if hi and r["t"] > hi:
            return False
        return not (a.conn and r.get("cookie", "").upper() != a.conn.upper())

    msgs = [m for m in msgs if keep(m)]
    if not msgs:
        print("no messages in that window")
        return
    tlo, thi = msgs[0]["t"], msgs[-1]["t"]
    refs = [r for r in refs if tlo <= r["t"] <= thi]
    calls = [c for c in calls if tlo <= c["t"] <= thi]
    meths = [m for m in meths if tlo <= m["t"] <= thi]

    print(f"log: {src}   {len(text.splitlines())} lines")
    print(f"window: {tlo:%H:%M:%S} -> {thi:%H:%M:%S}  ({(thi - tlo).seconds}s)")
    print(f"messages: {len(msgs)}   connections: {len(set(m['cookie'] for m in msgs))}")

    print(f"\n=== activity segments (idle gap >= {a.gap}s) ===")
    segs, cur = [], [msgs[0]]
    for prev, m in zip(msgs, msgs[1:]):
        if (m["t"] - prev["t"]).total_seconds() >= a.gap:
            segs.append(cur)
            cur = []
        cur.append(m)
    segs.append(cur)
    for s in segs:
        names = {c["name"].replace(".ProxyCaller", "")
                 for c in calls if s[0]["t"] <= c["t"] <= s[-1]["t"]}
        tag = ", ".join(sorted(names)[:3]) + ("..." if len(names) > 3 else "")
        print(f"  {s[0]['t']:%H:%M:%S}-{s[-1]['t']:%H:%M:%S}  {len(s):>4} msgs  "
              f"{(s[-1]['t'] - s[0]['t']).seconds:>4}s  {tag or '(no VI calls)'}")

    print("\n=== opcode vocabulary ===")
    op = OrderedDict()
    for m in msgs:
        key = (m["op"], m["name"])
        op.setdefault(key, [0, 0])
        op[key][0] += 1
        op[key][1] += m["len"]
    for (code, name), (n, tot) in sorted(op.items()):
        flag = "  <-- NEW" if code not in BASELINE_OPS else ""
        if name in WATCH_FOR:
            flag = "  <-- WATCHED (app/project-level method)"
        print(f"  {code:>3}  {name:<26} n={n:<4} payload={tot:>8}B{flag}")

    print("\n=== method IDs ===")
    for mid, name in sorted({(m["id"], m["name"]) for m in meths}):
        known = BASELINE_METHS.get(mid)
        print(f"  {mid}  {name}" + (f"  ({known})" if known else "  <-- NEW"))

    print("\n=== VI references opened ===")
    for path, n in Counter(r["path"] for r in refs).most_common():
        print(f"  {n:>3}x  {short(path)}")

    print("\n=== VI calls in order (elapsed to next) ===")
    ev = sorted([(c["t"], c["name"]) for c in calls])
    for i, (t, name) in enumerate(ev):
        nxt = ev[i + 1][0] if i + 1 < len(ev) else thi
        print(f"  {t:%H:%M:%S}  (+{(nxt - t).seconds:>4}s)  {name.replace('.ProxyCaller', '')}")

    print("\n=== payload heavyweights ===")
    for m in sorted(msgs, key=lambda x: -x["len"])[:10]:
        print(f"  {m['t']:%H:%M:%S}  {m['name']:<24} {m['len']:>8}B  uID=0x{m['uid']}")

    print("\n=== per-connection ===")
    per = defaultdict(lambda: [0, None, None])
    for m in msgs:
        e = per[m["cookie"]]
        e[0] += 1
        e[1] = e[1] or m["t"]
        e[2] = m["t"]
    for cookie, (n, first, last) in sorted(per.items(), key=lambda kv: kv[1][1]):
        print(f"  0x{cookie}  {n:>4} msgs  {first:%H:%M:%S}-{last:%H:%M:%S}")

    errs = [m for m in msgs if m["err"] != 0] + [r for r in refs if r["err"] != 0]
    print("\n=== errors ===")
    print(f"  {len(errs)} non-zero err fields" if errs else
          "  none in request headers - but replies are not logged, "
          "so this is NOT evidence of success")


if __name__ == "__main__":
    main()
