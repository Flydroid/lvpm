#!/usr/bin/env python3
r"""Read lvpm's own data: install records, the package cache, archive specs.

  install records  `<store>/.lvpm/installed/<package>.json` (the `Manifest` in
                   src/install.rs): name, version, target and every installed
                   file as an absolute path. A venv's records live in the venv;
                   a real LabVIEW's in `%ProgramData%\lvpm\<target key>\installed`.
  package cache    `<cache>/lvpm-cache.db` (src/cache.rs): one `packages` row
                   per imported or downloaded archive, its bytes in the
                   content store under `content/sha256/<2 hex>/<sha256>`.
  spec             the `spec` file inside every archive, the package's own INI
                   manifest (file groups, target dirs, dependencies, hooks).

The cache is resolved the way lvpm resolves it (src/config.rs):
`LVPM_CONFIG_CACHE`, else `cache` in `%LOCALAPPDATA%\lvpm\config.toml`, else
`%LOCALAPPDATA%\lvpm\cache`. An archive lvpm has not seen yet is imported with
`lvpm cache add <file or folder>`.
"""

import hashlib
import json
import os
import re
import sqlite3
import tomllib
import zipfile
from pathlib import Path

DEFAULT_LV = Path(r"C:\Program Files\National Instruments\LabVIEW 2026")

# Target Dir tokens that sit inside a LabVIEW (or a venv package) root, and the
# folder each names there (src/target.rs `expand`). The OS tokens point
# outside it and are not matched.
TOKEN_DIRS = {
    "<application>": "",
    "<vi.lib>": "vi.lib",
    "<vilib>": "vi.lib",
    "<user.lib>": "user.lib",
    "<userlib>": "user.lib",
    "<instr.lib>": "instr.lib",
    "<instrlib>": "instr.lib",
    "<menus>": "menus",
    "<resource>": "resource",
    "<help>": "help",
    "<project>": "project",
    "<templates>": "templates",
    "<examples>": "examples",
    "<fonts>": "fonts",
}


def _localappdata():
    return Path(os.environ.get("LOCALAPPDATA") or os.environ.get("TEMP") or ".")


def _env(key):
    """`LVPM_CONFIG_<KEY>`, else `lvpm_config_<key>`; empty counts as unset."""
    k = key.replace(".", "_")
    return os.environ.get(f"LVPM_CONFIG_{k.upper()}") or os.environ.get(f"lvpm_config_{k.lower()}") or None


def _config_file():
    p = _localappdata() / "lvpm" / "config.toml"
    if not p.is_file():
        return {}
    with p.open("rb") as f:
        return tomllib.load(f)


def cache_root():
    v = _env("cache") or _config_file().get("cache")
    return Path(v).resolve() if v else _localappdata() / "lvpm" / "cache"


def version_key(v):
    """Sort key for versions: '1.10.0.2' > '1.9.1.125', '1.1-1' handled."""
    return tuple(int(x) for x in re.findall(r"\d+", v))


def split_name(name_version):
    """'name' or 'name-version' -> (name, version or None)."""
    m = re.match(r"(.+?)-(\d[\w.\-]*)$", name_version)
    return (m.group(1), m.group(2)) if m else (name_version, None)


# --- package cache ---------------------------------------------------------

def _db(cache):
    p = cache / "lvpm-cache.db"
    if not p.is_file():
        raise SystemExit(f"no lvpm package cache at {p} — run lvpm once, or `lvpm cache add <folder>`")
    return sqlite3.connect(f"file:{p}?mode=ro", uri=True)


def cached_versions(name, cache=None):
    """Every version of `name` lvpm's cache holds, oldest first."""
    cache = cache or cache_root()
    with _db(cache) as db:
        rows = db.execute("SELECT DISTINCT version FROM packages WHERE name = ?", (name,)).fetchall()
    return sorted((r[0] for r in rows), key=version_key)


def archive(name_version, cache=None):
    """Path of the cached archive for 'name' or 'name-version' (newest when only
    a name), verified against its SHA-256; None when lvpm has not cached it."""
    cache = cache or cache_root()
    name, version = split_name(name_version)
    with _db(cache) as db:
        rows = db.execute("SELECT version, sha256 FROM packages WHERE name = ?", (name,)).fetchall()
    if version:
        rows = [r for r in rows if r[0] == version]
    for _ver, sha in sorted(rows, key=lambda r: version_key(r[0]), reverse=True):
        p = cache / "content" / "sha256" / sha[:2] / sha
        if p.is_file() and hashlib.sha256(p.read_bytes()).hexdigest() == sha:
            return p
    return None


def spec_text(vip):
    with zipfile.ZipFile(vip) as z:
        return z.read("spec").decode("utf-8", errors="replace")


# --- spec ------------------------------------------------------------------

def section(text, name):
    m = re.search(rf"^\[{re.escape(name)}\]\s*$(.*?)(?=^\[|\Z)", text, re.M | re.S)
    return m.group(1) if m else ""


def section_bodies(text, prefix):
    """Yield (section_name, body) for every [prefix...] section."""
    for m in re.finditer(rf"^\[({re.escape(prefix)}[^\]]*)\]\s*$(.*?)(?=^\[|\Z)", text, re.M | re.S):
        yield m.group(1), m.group(2)


def requires(text):
    """[(name, constraint)] from the spec's [Dependencies] Requires line."""
    m = re.search(r'^Requires="([^"]*)"', section(text, "Dependencies"), re.M)
    if not m or not m.group(1).strip():
        return []
    out = []
    for item in m.group(1).split(","):
        item = item.strip()
        if not item:
            continue
        mm = re.match(r"([a-z0-9_\-.&]+)\s*(>=|<=|==|>|<)?\s*([\d.]*)", item, re.I)
        out.append((mm.group(1), (mm.group(2) or "") + (mm.group(3) or "")) if mm else (item, ""))
    return out


def hooks(text):
    body = section(text, "Script VIs")
    return [ln.split("=", 1)[0].strip() for ln in body.splitlines()
            if "=" in ln and ln.split("=", 1)[1].strip().strip('"')]


def file_groups(text):
    """[(group name, Target Dir, [file rel, ...])] in spec order."""
    out = []
    for name, body in section_bodies(text, "File Group"):
        td = re.search(r'^Target Dir="([^"]*)"', body, re.M)
        if td:
            out.append((name, td.group(1), re.findall(r'^File \d+="([^"]*)"', body, re.M)))
    return out


def root_relative(target_dir, rel):
    """'vi.lib/addons/X/y.vi' for a file of a group, relative to the LabVIEW (or
    venv package) root; None for an OS token outside it."""
    m = re.match(r"(<[^>]+>)(.*)", target_dir)
    if not m or m.group(1).lower() not in TOKEN_DIRS:
        return None
    parts = [TOKEN_DIRS[m.group(1).lower()], m.group(2).strip("/\\"), rel]
    return "/".join(p.replace("\\", "/").strip("/") for p in parts if p)


# --- install records -------------------------------------------------------

def store_dir(where):
    """The install-record folder for `where`: a venv folder (`.lv-venv`,
    `.project`, ...), a project with a venv in it, a sandbox, or the records
    folder itself."""
    where = Path(where)
    if where.name == "installed" and where.is_dir():
        return where
    if (where / ".lvpm" / "installed").is_dir():
        return where / ".lvpm" / "installed"
    venv_dir = _env("venv.dir") or _config_file().get("venv", {}).get("dir") or ".lv-venv"
    for cand in dict.fromkeys([venv_dir, ".lv-venv", ".project"]):
        if (where / cand / ".lvpm" / "installed").is_dir():
            return where / cand / ".lvpm" / "installed"
    raise SystemExit(f"no lvpm install records under {where}")


def _rebase(files, venv, name):
    """A venv copied folder-for-folder keeps records that name the original
    venv's absolute paths; point them at `venv`'s own package folder instead.
    Paths outside a `<name>` folder (a real LabVIEW install) are left alone."""
    if not (venv / name).is_dir():
        return files
    out = []
    for f in files:
        parts = Path(f).parts
        i = next((k for k in range(len(parts) - 1, -1, -1) if parts[k].lower() == name.lower()), None)
        out.append(str(venv.joinpath(*parts[i:])) if i is not None else f)
    return out


def installed(where, names=None):
    """{name: record} for every package installed at `where`, or only `names`."""
    d = store_dir(where)
    venv = d.parent.parent
    out = {}
    for p in sorted(d.glob("*.json")):
        rec = json.loads(p.read_text(encoding="utf-8"))
        if names is None or rec["name"] in names:
            rec["files"] = _rebase(rec["files"], venv, rec["name"])
            out[rec["name"]] = rec
    if names:
        for n in names:
            if n not in out:
                raise SystemExit(f"{n} is not installed at {where} ({d})")
    return out


def installed_pairs(rec, vip):
    """Yield (root-relative path, archive member, installed Path) for every file
    of the archive the record installed. Matching is by root-relative suffix, so
    it needs no LabVIEW path: a venv's package root and a real LabVIEW's root
    end the same way."""
    by_suffix = {}
    for f in rec["files"]:
        p = Path(f)
        by_suffix.setdefault(p.name.lower(), []).append(p)
    with zipfile.ZipFile(vip) as z:
        members = set(z.namelist())
    for group, target_dir, files in file_groups(spec_text(vip)):
        for rel in files:
            member = f"{group}/{rel.replace(chr(92), '/')}"
            rr = root_relative(target_dir, rel)
            if rr is None or member not in members:
                continue
            tail = "/" + rr.lower()
            for p in by_suffix.get(Path(rr).name.lower(), []):
                if str(p).replace("\\", "/").lower().endswith(tail):
                    yield rr, member, p
                    break


# --- project manifest ------------------------------------------------------

def manifest_packages(path):
    """['name' or 'name-version', ...] from an lvpm.toml's [dependencies]; an
    exact version pins, a range or '*' takes the newest cached."""
    with Path(path).open("rb") as f:
        deps = tomllib.load(f).get("dependencies", {})
    return [f"{n}-{v}" if re.fullmatch(r"\d[\w.\-]*", v) else n for n, v in deps.items()]
