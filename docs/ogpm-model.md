# The OpenG Package Tools model

lvpm is a reimplementation of the **OpenG Package Tools** (OGPT) design: the
open, LGPL-licensed package system the OpenG community built for LabVIEW in
2002–2005, and the direct ancestor of VIPM. Everything VIPM later kept — the
zip-with-a-`spec` package, the INI package directory, `Target Dir` tokens,
`File Group N`, `Replace Mode`, `PreInstall`/`PostInstall` script VIs, the
`name-version-release` id — was designed and documented there, in public.
This document is the reference lvpm builds against, and the map from OGPT's
terms to lvpm's.

## Where it lives

| What | Where |
|---|---|
| Project site, manual, design documents | <https://ogpm.sourceforge.net/> |
| Design document index (OGPT Design TOC) | <https://ogpm.sourceforge.net/design/OGPT%20Design%20TOC.html> |
| Spec file format | <https://ogpm.sourceforge.net/design/Package%20Spec%20File.htm> |
| Path root keywords | <https://ogpm.sourceforge.net/design/Path%20Roots.htm> |
| OGPI manual (v0.5, Nov 2002) | <https://ogpm.sourceforge.net/OGPI/index.html> |
| SourceForge project (“OpenG Commander”, LGPL 2.0) | <https://sourceforge.net/projects/ogpm/> |
| Full CVS history, one zip | <https://sourceforge.net/code-snapshots/cvs/o/og/ogpm.zip> |
| Releases (ogpi, ogpi_api, openg_commander, openg_package_builder) | <https://sourceforge.net/projects/ogpm/files/> |

The CVS snapshot holds the LabVIEW source of every component (`OGPI/`,
`OGPM/`, `ogpb/`, `ogpi20/`, `commander/`), the design documents under
`spec_dev/Design Docs/` and `website/design/`, the original proposal
(`spec_dev/installer.txt`, Rolf Kalbermatter, 16 Sept 2002), and a cache of
real 2004 packages' `.spec` files plus the `openg.ogpd` directory that listed
them. Text files are RCS `,v`; the head revision is the first `text @…@`
block.

Authors of record: Jim Kring (OGPI, OGPM, design documents), Rolf
Kalbermatter (original proposal, lvzip), Konstantin Shifershteyn (Package
Builder), Michael Aivaliotis and others (OpenG Commander). The last
release is OpenG Commander 2.0 alpha 3, March 2013. VIPM, from JKI, took over
the ecosystem from there; its `.vip` format is the OGPT `.ogp` format with a
renamed section and a fourth version component.

## Architecture

OGPT is layered, and the layering is the model:

**OGPI — Package Installer.** The base. Knows one package at a time: parse the
`spec`, check OS / LabVIEW version / LabVIEW system requirements, unzip to a
temp dir, run `PreInstall`, copy each `File Group` to its `Target Dir`, run
`PostInstall`. Uninstall is the reverse: `PreUninstall`, delete the files,
`PostUninstall`. It pays *no* attention to package dependencies or conflicts.

**OGPM — Package Manager.** Sits on top. Owns everything that spans packages:
the dependency and conflict tree, the package directories and repositories
(where packages come from), the installed-package database, query and
verification. When OGPM is present, OGPI delegates to it; the OGPI docs say a
double-clicked `.ogp` goes to OGPM "if installed, otherwise to the OGPI, which
will pay no attention to package dependencies".

**OGPB — Package Builder.** A developer front end that writes a `spec` from a
simpler build file (`.ogb`/`.ogpb`), so packagers never hand-edit the spec.
Can use OGPM's services for automatic dependency discovery (`AutoReqProv`).

**DSCM — Development System Configuration Manager.** Planned, never shipped:
export what is installed and its dependency tree as a configuration file,
import it on another machine and have the packages downloaded and installed;
switch between configurations for different projects.

lvpm collapses OGPI and OGPM into one binary but keeps the seam: `spec.rs`,
`target.rs`, `install.rs` are OGPI; `index.rs`, `project.rs`, `venv.rs` and
the resolver are OGPM. The manifest and venv are lvpm's DSCM.

## The package

A package is a zip with the extension `.ogp` (`.vip` in VIPM's dialect),
containing:

```
package_name.spec       # the manifest; VIPM shortened the name to `spec`
<icon>                  # .bmp/.png named by [Description] Icon
File Group 0/           # one payload tree per file group
File Group 1/
…
```

The archive is deliberately plain PKZIP so any zip tool can open it; OGPT
used its own `lvzip` (the OpenG Zip Tools) only because LabVIEW had no zip
primitive.

### The spec

An INI file. Sections and the keys that matter for installing:

| Section | Keys | Notes |
|---|---|---|
| `[Package Name]` | `Name`, `Version`, `Release` | id is `Name-Version-Release`, e.g. `oglib_error-2.0-1`. No dashes or spaces in `Version`; `Release` is the *packaging* revision, reset to 1 on a new `Version`. VIPM renamed the section `[Package]`, folded `Release` into a fourth version component and added `Display Name`. |
| `[Description]` | `Description`, `Summary`, `License`, `Copyright`, `Distribution`, `Icon`, `Vendor`, `URL`, `Packager` | metadata; `Distribution` groups packages ("OpenG Toolkit"). |
| `[Dependencies]` | `Requires`, `Conflicts`, `AutoReqProv` | `Requires="bar >= 2.7-4; baz = 2.1-1"` — semicolon list, operators `< > = >= <=`, release optional. `Conflicts` has the same grammar. |
| `[Platform]` | `Exclusive_LabVIEW_Version`, `Exclusive_LabVIEW_System`, `Exclusive_OS`, `Exclusive_Arch`, and `Exclude_*` twins | package-wide gates; `Exclusive_LabVIEW_Version= >=6.0` or a list `6.0,6.1,7.0`. |
| `[Script VIs]` | `PreInstall`, `PostInstall`, `PreUninstall`, `PostUninstall`, `Verify`, `PreBuild`, `PostBuild`, `Source Dir` | hook VIs shipped in the archive, **run from the temporary extraction directory**, not from an installed location. Early drafts split these into `[Build-Time…]`, `[Install and Erase-Time…]`, `[Verification-Time…]` sections. |
| `[Files]` | `Num File Groups`, `Target Dir`, `Documentation`, `DocGroups` | `Target Dir` here is the root for relative group targets, default `<labview>`. |
| `[File Group N]` | `Source Dir`, `Target Dir`, `Replace Mode`, `Num Files`, `File 0…`, `Source Dir to LLB`, plus per-group `Exclusive_*`/`Exclude_*` | `Replace Mode` is `Never`, `Always` or `If Newer`. A group whose platform gate fails is silently not installed — that is how one `.ogp` serves every OS and LabVIEW version. |

A real one, `oglib_error-2.0-1.spec` from the 2004 cache, trimmed:

```ini
[Package Name]
Name=oglib_error
Version=2.0
Release=1

[Dependencies]
Requires="ogrsc_dynamicpalette >= 2.0"
AutoReqProv=FALSE

[Platform]
Exclusive_LabVIEW_Version= >=6.0
Exclusive_OS=All

[Files]
Num File Groups=2

[File Group 0]
Source Dir="built/error"
Target Dir=<user.lib>/_OpenG.lib/error
Replace Mode=Never
Num Files=4
File 0=dir.mnu
File 1="error.llb/Build Error Cluster__ogtk.vi"
…
```

### Path root keywords

`Target Dir` starts with a keyword the installer resolves against the target
LabVIEW (design doc "Path Roots"):

| Keyword | Resolves to |
|---|---|
| `<application>` | the LabVIEW directory |
| `<vi.lib>`, `<user.lib>`, `<instr.lib>` | the libraries, honouring `labview.ini` overrides |
| `<project>` | `project/` — the Tools menu |
| `<file>` | `wizard/` — the File menu |
| `<help>`, `<examples>`, `<resource>` | the obvious folders |
| `<menus>` | `menus/ActivePalette` — the active palette set |
| `<preferences>` | `labview.ini` (Mac/Unix equivalents) |
| `<temp>`, `<os>`, `<system>`, `<home>` | machine locations |
| `<OpenG.lib>`, `<OpenG palette>` | OpenG's own tree, later moved under `user.lib/_OpenG.lib` |

VIPM dropped the dots (`<vilib>`, `<userlib>`) and grew the `<OS …>` family.
lvpm's `target.rs` speaks VIPM's spelling because that is what the published
corpus contains.

## Distribution

**Package Directory** — a `.ogpd` file: an INI listing packages and where to
get them. This is the format `www.jkisoft.com/packages/jkisoft.ogpd` still
serves today, unchanged in shape; `index.vipr` is the same thing with an MD5
per entry.

```ini
[Self]
Self.Name=OpenG Package Directory
Release.Number=4
Release.Date=Sep 19th, 2004
Self.URL=file:///C/packages/openg/openg.ogpd
Creator.Name=OpenG.org

[Package oglib_error-2.0-1]
Package.URL=oglib_error-2.0-1.ogp      ; relative to the .ogpd, or absolute
Icon.URL=oglib_error-2.0-1.bmp
Spec.URL=oglib_error-2.0-1.spec        ; the spec published beside the package,
                                       ; so dependencies resolve without a download
```

**Directory list** — `Root.ogpd` (`[Directory N] Directory.URL=…`), earlier
`directories.txt`, one URL per line, searched in order. The **local
repository** is always checked first.

**Local repository** — "merely a directory that contains packages"; since
packages are named `name-version-release.ogp`, a directory listing *is* the
index.

**Package cache** — `ogpm.ini` `Package Cache Folder`: downloaded `.ogp`s,
their specs and icons, kept so a refresh or reinstall does not fetch again.

**Mirrors** — a list of SourceForge mirrors with a preference order, because
the repository was hosted there.

## The installed-package database

Under `<labview>/project/OGPM/` (later `resource/OpenG/Commander/db/`):

- `_repository/` — a copy of every installed package file.
- `_db/` — each installed package's spec and icon. **A package is installed
  iff its spec is in `_db`.** Queries (what depends on X, which package owns
  this VI, what does X require) read the specs there and nothing else.
- `namespace.txt` (proposed) — every VI name on the system, so a package that
  would duplicate a VI name is caught before install.

## Sequences

**Install (OGPM):** read spec → for each unmet `Requires`, push this package
and install the dependency first → for each `Conflicts` hit, uninstall it →
copy the package to `_repository`, its spec and icon to `_db` → hand to OGPI.
"If the dependency tree has been mapped, everything may be downloaded first."

**Install (OGPI):** check OS / LabVIEW gates → extract to temp → `PreInstall`
→ copy file groups honouring `Replace Mode` → `PostInstall`.

**Uninstall:** find dependees, uninstall them first → `PreUninstall` → delete
the group files → `PostUninstall` → remove spec and icon from `_db`.

**Upgrade:** uninstall the old package, install the new one. Nothing cleverer.

**Verify:** compare installed files against the package's record (and run the
package's `Verify` VI if it has one); report anomalies; user reinstalls.

Mass compile was on OGPI's to-do list and never landed: "Option in Spec Files
to Mass Compile File Groups … load individual File Group VIs and then save?"
That last clause is exactly lvpm's relink pass.

## Naming and versioning rules

- Package id `PkgName-Major.Minor-Release`, e.g. `ogtkstring-1.2-4`.
- Bump **major** when a release is incompatible: it removes or renames a VI,
  changes a connector pane, adds a required terminal, or changes behaviour.
  Otherwise bump minor.
- A new major should not share a namespace with the old one, so several
  majors can coexist: prefix VIs `PkgName-Major VI Name.vi`. OpenG settled on
  the `__ogtk` suffix instead, and later packages commonly suffix `__<libname>`.

## OGPT → lvpm

| OGPT | lvpm | State |
|---|---|---|
| OGPI spec parser, `[Package Name]` + `Release` | `spec.rs` — reads both dialects | done |
| OGPI installer: gates, extract, hooks, copy, `Replace Mode` | `install.rs`; `If Newer` still "write when absent" | done / partial |
| Script VIs run from the temp extraction dir | hook VIs extracted and run over VI Server | done |
| Path root keywords | `target.rs` (VIPM spelling) | done |
| Per-file-group `Exclusive_OS` / `Exclusive_LabVIEW_Version` | package-level `lv_gate` only; `<OS …>` groups skipped in a venv | partial |
| Package Directory `.ogpd` / `Root.ogpd` | `index.rs`; `lvpm.toml [sources]` | done |
| Local repository, checked first | `--repo <DIR>`, `[sources] local` | done |
| Package cache | roadmap | planned |
| Installed-package `_db` | per-package install manifest, `lvpm list` | done |
| `Requires` with `>=` | `Requirement.min`; other operators and `Conflicts` unparsed | partial |
| Conflict handling | roadmap | planned |
| Upgrade = uninstall then install | — | planned |
| Package Query (owner of a VI, dependees, tree) | `tools/deptree.py`; roadmap `lvpm tree` | planned |
| Global VI namespace check | roadmap `lvpm check` | planned |
| Package Verification, `Verify` VI | roadmap `lvpm verify` | planned |
| DSCM: export/import a configuration, per-project switching | `lvpm.toml` + venv; lockfile on the roadmap | partial |
| OGPB: build a spec from a simpler build file, `AutoReqProv` | roadmap: an lvpm package format built without LabVIEW | planned |
| `Source Dir to LLB`, dir→LLB/EXE at install | roadmap: PPL support | planned |
| `ogpi_shell.vbs … /install /uninstall /upgrade` | the `lvpm` CLI | done |
| Mass-compile-by-load-and-save (to-do, never built) | `relink.rs` + `Relink Package.vi` | done |
