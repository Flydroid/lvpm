"""Check every link object's qualifier chain in a venv store against the ownership
the .lvlib/.lvclass files declare (member item types only; parents and friends are
not members). Expected chain = target's owner chain minus the common prefix with the
caller's chain, outermost first. Reports WRONG prefixes (elements that are not in
the expected chain) separately from MISSING ones (a packaged VI lacking a qualifier).
Usage: python tools/linkfix/qualcheck.py <store .project> [package-filter]"""
import sys, re, struct
from pathlib import Path
from collections import Counter, defaultdict
sys.path.insert(0, str(Path(__file__).parent))
from linkobjs import li_objects, parse_pth
from linker_diff import rsrc_blocks
store = Path(sys.argv[1]); filt = sys.argv[2] if len(sys.argv) > 2 else ""
TOK = {"<vilib>": "vi.lib", "<userlib>": "user.lib", "<instrlib>": "instr.lib", "<resource>": "resource", "<menus>": "menus", "<examples>": "examples"}
MEMBER = {"VI", "LVClass", "Library", "Class Private Data", "Control"}
files = {}   # lower logical -> (logical, physical)
for pkg in sorted(p for p in store.iterdir() if p.is_dir() and not p.name.startswith(".")):
    root = pkg / "1"
    if not root.is_dir(): continue
    for p in root.rglob("*"):
        if p.is_file():
            logical = str(p.relative_to(root)).replace("\\", "/")
            files[logical.lower()] = (logical, p, pkg.name)
def join_rel(dirlogical, rel):
    parts = [s for s in dirlogical.split("/") if s]
    for seg in rel.split("/"):
        if seg in ("", "."): continue
        if seg == "..":
            if not parts: return None
            parts.pop()
        else: parts.append(seg)
    return "/".join(parts)
def tok_to_logical(comps):
    if not comps: return None
    t = TOK.get(comps[0].lower())
    if t is None: return None
    return "/".join([t] + comps[1:])
owner = {}
for low, (logical, phys, pkgname) in list(files.items()):
    if not (low.endswith(".lvlib") or low.endswith(".lvclass")): continue
    xml = phys.read_bytes().decode("utf-8", "replace")
    for m in re.finditer(r'<Item\s([^>]*)>', xml):
        attrs = dict(re.findall(r'(\w+)="([^"]*)"', m.group(1)))
        ty, url = attrs.get("Type"), attrs.get("URL")
        if ty not in MEMBER or not url: continue
        url = url.replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&").replace("\\", "/")
        if url.startswith("/"):
            comps = url[1:].split("/"); member = tok_to_logical(comps)
        else:
            member = join_rel(logical, url)   # library file acts as a folder
        if member is None: continue
        if member.lower() == low or member.lower() not in files: continue
        owner[member.lower()] = logical
# LLB members: lvpm unpacks .llb as folders; the openg lvlib inside declares members by URL too (handled above).
def chain(logical):
    out, cur = [], logical.lower()
    while cur in owner and len(out) < 8:
        o = owner[cur]
        if o in out: break
        out.append(o); cur = o.lower()
    return out
def base(l): return l.rsplit("/", 1)[-1]
API = {"VIPI", "FPPI", "DDPI", "DyOM", "PNOM", "TCPI", "LVCC", "VRPI"}
stats = Counter(); ex = defaultdict(list); per_pkg = defaultdict(Counter)
for low, (logical, phys, pkgname) in files.items():
    if filt and filt.lower() not in pkgname.lower(): continue
    if Path(logical).suffix.lower() not in {".vi", ".ctl", ".vim", ".vit", ".ctt"}: continue
    data = phys.read_bytes()
    if data[:4] != b"RSRC" or data[8:12] not in (b"LVIN", b"LVCC"): continue
    try: ver, res = li_objects(data)
    except Exception as e: stats["unparseable"] += 1; continue
    ours = [base(l) for l in reversed(chain(logical))]
    for ident, objs in res.items():
        for id_, qual, (tp, comps), flag, tail, *_ in objs:
            k = id_.decode()
            if k in ("VILB", "DSDS", "DSSV", "NEXF", "DNVA", "DNDA") or not qual: continue
            if tp != 0 or not comps or not comps[0].startswith(b"<"): continue   # only token paths resolve here
            target = tok_to_logical([c.decode("latin-1") for c in comps])
            if target is None: continue
            tl = target.lower()
            if tl not in files:
                # class private data ctl lives inside the class file
                par = target.rsplit("/", 1)[0]
                if par.lower().endswith(".lvclass") and par.lower() in files:
                    owner[tl] = files[par.lower()][0]
                else: stats["target not in store"] += 1; continue
            names = [base(l) for l in reversed(chain(target))]
            if k in API: names.append(base(target))
            common = 0
            for a, b in zip(names, ours):
                if a.lower() == b.lower(): common += 1
                else: break
            expected = names[common:]
            actual = [q.decode("latin-1") for q in qual[:-1]]
            if [a.lower() for a in actual] == [e.lower() for e in expected]:
                stats["ok"] += 1; continue
            if len(actual) < len(expected) and [a.lower() for a in actual] == [e.lower() for e in expected[len(expected) - len(actual):]]:
                cat = f"MISSING prefix ({ident} {k})"
            else:
                cat = f"WRONG prefix ({ident} {k})"
            stats[cat] += 1; per_pkg[pkgname][cat.split(" (")[0]] += 1
            if len(ex[cat]) < (60 if filt else 5):
                ex[cat].append(f"{pkgname}: {logical} -> {target}: actual={actual} expected={expected} (caller chain {ours})")
print(f"## qualifier check of {store}")
for k, v in sorted(stats.items(), key=lambda kv: -kv[1]): print(f"   {v:>6} {k}")
print("\n## per package")
for p, c in sorted(per_pkg.items()): print(f"   {p:60s} {dict(c)}")
print("\n## examples")
for k, lst in sorted(ex.items()):
    print(f"\n{k}")
    for e in lst: print("   ", e)
