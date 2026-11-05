#!/usr/bin/env python3
"""Walk a package's dependency closure using the specs in lvpm's package cache.

Every archive carries its own INI manifest (`spec`) with a [Dependencies]
Requires line, so the dependency graph can be resolved offline from what lvpm
has already cached (see lvpmdata.py). The newest cached version of each
dependency is taken; `lvpm install` applies the real constraints.

    python tools/linkfix/deps.py bjustice_lib_rocketplot
    python tools/linkfix/deps.py oglib_file-6.0.2.28

Also reports which packages declare [Script VIs] hooks (PostInstall,
PreUninstall, ...), since those are the ones that need a LabVIEW process at
install time rather than a plain file copy.
"""

import argparse
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from lvpmdata import archive, hooks, requires, spec_text, split_name  # noqa: E402


def main():
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("package", help="name or name-version")
    a = ap.parse_args()

    root, _ = split_name(a.package)
    seen, order, missing = {}, [], set()
    queue = [a.package]
    while queue:
        item = queue.pop(0)
        name, _ = split_name(item)
        if name in seen:
            continue
        vip = archive(item)
        if vip is None:
            missing.add(name)
            seen[name] = None
            continue
        text = spec_text(vip)
        version = next((ln.split("=", 1)[1].strip('"') for ln in text.splitlines()
                        if ln.startswith("Version=")), "?")
        seen[name] = (f"{name}-{version}", requires(text), hooks(text))
        order.append(name)
        queue.extend(d for d, _ in seen[name][1] if d not in seen)

    print(f"=== dependency closure of {root} (from lvpm's package cache) ===")
    print(f"resolved: {len(order)}   not cached: {len(missing)}\n")
    for i, name in enumerate(order, 1):
        stem, reqs, hk = seen[name]
        tag = f"   [Script VIs: {', '.join(hk)}]" if hk else ""
        print(f"{i:>3}. {stem}  ({len(reqs)} deps){tag}")

    if missing:
        print(f"\n--- referenced but not in lvpm's cache ({len(missing)}) ---")
        for m in sorted(missing):
            print("   ", m)

    with_hooks = [n for n in order if seen[n][2]]
    print(f"\n=== packages carrying install hooks ({len(with_hooks)}) ===")
    for n in with_hooks:
        print(f"   {seen[n][0]}: {', '.join(seen[n][2])}")
    if not with_hooks:
        print("   none - this closure installs by file copy alone")


if __name__ == "__main__":
    main()
