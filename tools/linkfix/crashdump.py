#!/usr/bin/env python3
r"""Summarise a LabVIEW crash dump: the exception, and the frames that repeat.

A stack overflow in LabVIEW's compiler is an endless recursion, and in the dump
it shows as a handful of return addresses repeated thousands of times on the
faulting thread's stack. Counting code pointers on that stack (module+offset)
identifies the cycle without symbols, and two dumps with the same cycle are the
same crash. The linkfix crash of 2026-09/10 (NovaXC on LabVIEW 2026) is
`0xC00000FD` with an 8-frame cycle repeated ~18,550 times, two of its frames
being `dfir_26_3.dll+0x1d5968` and `dfir_26_3.dll+0x1d888e`.

    python tools/linkfix/crashdump.py <file.dmp>
    python tools/linkfix/crashdump.py --venv C:\Git\venv-bisect2      # newest dump of that venv

A venv LabVIEW writes its reports under the venv's own data folder
(`<venv>\.lvpm\data\LVInternalReports\...\reports\*.dmp`); a ~16 MB dump is a
real crash, 1-4 MB ones are warnings (DWarn/NIASR). Needs `pip install minidump`.
"""

import argparse
import sys
from collections import Counter
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from lvpmdata import store_dir  # noqa: E402


def newest_dump(venv):
    data = store_dir(venv).parent / "data" / "LVInternalReports"
    dumps = sorted(data.rglob("*.dmp"), key=lambda p: p.stat().st_mtime)
    if not dumps:
        raise SystemExit(f"no dumps under {data}")
    return dumps[-1]


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("dump", nargs="?", type=Path)
    ap.add_argument("--venv", type=Path, help="take the newest dump of this venv")
    ap.add_argument("--top", type=int, default=12, help="how many repeating frames to list")
    a = ap.parse_args()
    if not a.dump and not a.venv:
        ap.error("give a .dmp or --venv")
    dump = a.dump or newest_dump(a.venv)

    from minidump.minidumpfile import MinidumpFile

    mf = MinidumpFile.parse(str(dump))
    reader = mf.get_reader()
    exc = mf.exception.exception_records[0]
    rec = exc.ExceptionRecord
    mods = [(m.baseaddress, m.endaddress, m.name.split("\\")[-1]) for m in mf.modules.modules]

    def where(addr):
        for lo, hi, n in mods:
            if lo <= addr < hi:
                return f"{n}+0x{addr - lo:x}"
        return None

    print(f"{dump}  ({dump.stat().st_size / 1e6:.1f} MB)")
    print(f"exception 0x{rec.ExceptionCode_raw:08X} at {where(rec.ExceptionAddress)} (thread {exc.ThreadId})")
    thread = next(t for t in mf.threads.threads if t.ThreadId == exc.ThreadId)
    data = reader.read(thread.Stack.StartOfMemoryRange, thread.Stack.MemoryLocation.DataSize)
    hits = Counter()
    for off in range(0, len(data) - 7, 8):
        w = where(int.from_bytes(data[off:off + 8], "little"))
        if w and (".exe" in w.lower() or ".dll" in w.lower()):
            hits[w] += 1
    print(f"stack {len(data)} bytes, {sum(hits.values())} code pointers; most repeated:")
    for w, n in hits.most_common(a.top):
        print(f"  {n:>7}  {w}")


if __name__ == "__main__":
    main()
