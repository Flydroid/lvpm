#!/usr/bin/env python3
r"""Predict which packaged VIs LabVIEW will want to save after loading them.

Measured 2026-09-17: closing the NovaXC project raised a save dialog for 49
venv VIs, and every one of them had only ADDED linker-table entries. What a
packaged VI lacks, and LabVIEW fills in on load:

  owner   the `VILB` (VI-to-owning-library) path is relative
          (`["", "", "X.lvlib"]`) or empty instead of `<vilib>`-token absolute
  qual    a subVI link whose target is a library member lacks the library
          qualifier in its qualified name (`[lib.lvlib, sub.vi]`) and/or has a
          zero-filled `VILSPathRef`
  abs     a path is a drive-letter absolute from the build machine

This reads the link tables with pylabview (`pip install pylabview`), whose
`LVlinkinfo.py` is the per-version layout spec, so it needs no LabVIEW. It
checks the rule against the whole set at once instead of one IDE close per
project. Compare against what the IDE actually dirtied with --observed.

    python tools/linkfix/predict_dirty.py --manifest lvpm.toml
    python tools/linkfix/predict_dirty.py --installed C:\Git\venv-novaxc --observed C:\Git\venv-novaxc\.project
    python tools/linkfix/predict_dirty.py jki_lib_unicode --observed C:\Git\venv-novaxc\.project

The packaged VIs come from lvpm's package cache (see lvpmdata.py).
"""

import argparse
import io
import re
import sys
import types
from collections import Counter, defaultdict
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from audit import archive_files, package_names  # noqa: E402
from lvpmdata import archive  # noqa: E402

VI_EXT = {".vi", ".vim", ".vit", ".ctl", ".ctt"}
LIB_EXT = {".lvlib", ".lvclass"}
LINK_BLOCKS = (b"LIvi", b"LIbd", b"LIfp", b"LIds")

# pylabview reads its options off an argparse namespace; these are readRSRC's defaults.
PO = types.SimpleNamespace(
    verbose=0, xml=None, rsrc="x.vi", filebase="x", print_map=None, typedesc_list_limit=4095,
    array_data_limit=4095, store_as_data_above=4095, keep_names=False, force_recover_section=False,
    drop_section=[], password=None, raw_connectors=False, textcp="mac_roman")


def load_vi(name, data):
    """A parsed pylabview VI, or None when the file is not one it can read."""
    import contextlib
    from pylabview.LVrsrcontainer import VI
    fh = io.BytesIO(data)
    fh.name = name
    # pylabview prints parse warnings for blocks we do not use (VITS); keep the output ours.
    with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
        try:
            return VI(PO, rsrc_fh=fh, text_encoding="mac_roman")
        except Exception:
            return None


def library_members(files):
    """{member file basename (lower): owning library basename} from every .lvlib/.lvclass."""
    owner = {}
    for rel, data in files.items():
        if Path(rel).suffix.lower() not in LIB_EXT:
            continue
        lib = Path(rel).name
        text = data.decode("utf-8", errors="replace")
        for url in re.findall(r'Type="(?:VI|Class|Library|Control)"\s+URL="([^"]+)"', text):
            url = url.replace("&lt;", "<").replace("&gt;", ">")
            owner[Path(url.replace("\\", "/")).name.lower()] = lib
    return owner


def link_objects(vi):
    """(block ident, link object) for every entry in the VI's link tables."""
    for ident in LINK_BLOCKS:
        blk = vi.blocks.get(ident)
        if blk is None:
            continue
        for sec in blk.sections.values():
            for lo in getattr(sec, "content", []):
                yield ident, lo


def is_token_absolute(pathref):
    return (pathref is not None and getattr(pathref, "tpval", 1) == 0
            and pathref.content and pathref.content[0].startswith(b"<"))


def is_relative(pathref):
    return pathref is not None and getattr(pathref, "tpval", 0) == 1


def is_drive_absolute(pathref):
    if pathref is None or getattr(pathref, "tpval", 1) != 0 or not pathref.content:
        return False
    head = pathref.content[0]
    return len(head) == 1 and head.isalpha()


def predict(vi, owner_of, in_llb):
    """Set of reasons LabVIEW would rewrite this VI's link tables.

    Measured: OpenG lvdata VIs (inside lvdata.llb) link to their owner and to
    each other with same-directory relative paths, were loaded by the NovaXC
    project, and were NOT dirtied — relative links inside an LLB are the
    canonical form. The same relative form in a VI that sits in a plain folder
    under vi.lib (Empty Cluster.vi, Bytes In Rune.vi) IS rewritten to the
    `<vilib>` token form. So a relative path only counts outside an LLB.
    """
    reasons = set()
    for ident, lo in link_objects(vi):
        pr = getattr(lo, "linkSavePathRef", None)
        if lo.ident == b"VILB":
            if not is_token_absolute(pr) and not (in_llb and is_relative(pr)):
                reasons.add("owner")
            continue
        if pr is not None and is_drive_absolute(pr):
            reasons.add("abs")
        if pr is not None and is_relative(pr) and not in_llb:
            reasons.add("rel")
        # Same rule for the qualifier: OpenG's LLB-internal relative links to
        # library members carry no qualifier and LabVIEW left them alone.
        if pr is not None and pr.content and not (in_llb and is_relative(pr)):
            target = pr.content[-1].decode("latin-1").lower()
            if target in owner_of:
                qual = getattr(lo, "linkSaveQualName", []) or []
                vils = getattr(lo, "viLSPathRef", None)
                if len(qual) < 2:
                    reasons.add("qual")
                elif vils is not None and not getattr(vils, "content", None):
                    reasons.add("qual")
    return reasons


def raw_predict(data, in_llb):
    """Fallback when pylabview cannot parse the VI: scan the link blocks for
    raw PTH0 records and apply the path-type part of the rule only."""
    import struct
    from linker_diff import rsrc_blocks
    reasons = set()
    blocks = rsrc_blocks(data) or {}
    for ident in ("LIvi", "LIbd", "LIfp", "LIds"):
        for sec in blocks.get(ident, []):
            i = 0
            while (i := sec.find(b"PTH0", i)) >= 0:
                try:
                    ptype, count = struct.unpack_from(">HH", sec, i + 8)
                except struct.error:
                    break
                if count <= 128:
                    if ptype == 1 and not in_llb:
                        reasons.add("rel")
                    elif ptype == 0 and count and sec[i + 13:i + 14].isalpha() and sec[i + 12] == 1:
                        reasons.add("abs")
                i += 4
    return reasons


def observed_dirty(venv, pkg, files):
    """Archive paths of `pkg` whose venv copy differs byte-wise from the packaged
    one: what the IDE (or a hook) saved. Matched by the full relative path — a
    basename match reported 434 "observed" for OpenGDS, whose classes reuse
    names like ClassAttributes.ctl and Serialize.vi, when only 66 venv files
    had actually changed."""
    root = Path(venv) / pkg / "1"
    if not root.is_dir():
        return None
    out = set()
    for rel, data in files.items():
        if Path(rel).suffix.lower() not in VI_EXT or ".llb/" in rel.lower():
            continue
        p = root / rel
        if p.is_file() and p.read_bytes() != data:
            out.add(rel.lower())
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("packages", nargs="*")
    ap.add_argument("--manifest", type=Path, help="every dependency an lvpm.toml lists")
    ap.add_argument("--installed", type=Path, metavar="WHERE",
                    help="every package installed in a venv (or a project with one)")
    ap.add_argument("--observed", type=Path, metavar="VENV_PROJECT_DIR",
                    help="a venv's .project dir the IDE has saved into; compare prediction with it")
    ap.add_argument("--list", action="store_true", help="name every predicted VI")
    a = ap.parse_args()

    names = package_names(a.packages, a.manifest, a.installed)
    if not names:
        ap.error("give package names, --manifest or --installed")
    vips = {n: archive(n) for n in names}
    vips = {n: v for n, v in vips.items() if v}

    # Library membership is a property of the whole set: a VI's subVI may live in another package.
    archives = {n: archive_files(v) for n, v in vips.items()}
    owner_of = {}
    for files in archives.values():
        owner_of.update(library_members(files))

    grand = Counter()
    w = max(len(n) for n in vips)
    print(f"{'package':{w}}  {'VIs':>5} {'predict':>7} {"owner":>5} {"qual":>4} {"rel":>4} {"abs":>4}   observed / predicted-of-observed / missed")
    for n, files in archives.items():
        pkg = n.rsplit("-", 1)[0] if re.search(r"-\d", n) else n
        per = Counter()
        predicted = {}
        for rel, data in files.items():
            if Path(rel).suffix.lower() not in VI_EXT or data[:4] != b"RSRC":
                continue
            in_llb = ".llb/" in rel.lower()
            vi = load_vi(Path(rel).name, data)
            per["vis"] += 1
            if vi is None:
                # pylabview gave up on some other block (a .NET type descriptor in
                # process_exit_callback.vi). The link tables are still readable
                # as raw PTH0 records, and the rule only needs the path type.
                per["unparsed"] += 1
                r = raw_predict(data, in_llb)
            else:
                r = predict(vi, owner_of, in_llb)
            if r:
                predicted[rel.lower()] = r
                per["predict"] += 1
                for k in r:
                    per[k] += 1
        grand.update(per)
        obs_txt = ""
        if a.observed:
            obs = observed_dirty(a.observed, pkg, files)
            if obs is not None:
                hit = obs & set(predicted)
                missed = sorted(obs - set(predicted))
                obs_txt = f"{len(obs):>3} / {len(hit):>3} / {len(missed)}" + (f"  MISSED: {missed[:4]}" if missed else "")
                grand["observed"] += len(obs)
                grand["hit"] += len(hit)
        print(f"{n:{w}}  {per['vis']:>5} {per['predict']:>7} {per["owner"]:>5} {per["qual"]:>4} {per["rel"]:>4} {per["abs"]:>4}   {obs_txt}")
        if a.list:
            for f, r in sorted(predicted.items()):
                print(f"      {','.join(sorted(r)):12} {f}")
    print(f"\n{'total':{w}}  {grand['vis']:>5} {grand['predict']:>7} {grand["owner"]:>5} {grand["qual"]:>4} {grand["rel"]:>4} {grand["abs"]:>4}"
          + (f"   observed {grand['observed']}, predicted {grand['hit']}, recall {grand['hit']/grand['observed']:.0%}" if grand["observed"] else ""))
    if grand["unparsed"]:
        print(f"unparsed by pylabview: {grand['unparsed']}")


if __name__ == "__main__":
    main()
