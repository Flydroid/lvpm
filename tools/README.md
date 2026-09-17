# tools

Analysis scripts used to work out what VIPM actually asks LabVIEW to do during a
package install. Not part of the lvpm build — plain Python 3, no dependencies.

`Relink Package.vi` is the exception: it is not analysis, it is shipped code —
`include_bytes!`-ed into the `lvpm` binary from here, so editing it and
rebuilding is all it takes to ship a new one.

## `Relink Package.vi`

The load-resolve-save cycle, as a VI, because that is where it belongs — the
work is LabVIEW's own and needs no wire protocol beyond starting it. Its whole
interface is one control and one indicator:

| | name | type |
|---|---|---|
| in | `Folder to relink` | string |
| out | `Done` | boolean |

`lvpm install` runs it over each newly installed package once every package's
files are on disk, one call per package folder, and polls `Done` while it works
(`src/relink.rs`). `lvpm relink <package>` runs the same pass on its own, from
what the install manifest recorded.

It travels inside the executable and is extracted to
`%LOCALAPPDATA%\lvpm\tools\<md5>\Relink Package.vi` the first time a relink
runs — content-keyed, so a new build never overwrites a copy LabVIEW still has
open. `LVPM_RELINK_VI` points lvpm at a VI on disk instead, for trying a change
without rebuilding.

The unit of work is a package folder, taken from the `Target Dir` its spec asked
for — `<vi.lib>/10X Engineering/AnimateX`, not `vi.lib`, and not each
subdirectory separately.

## `parse_viserver_log.py`

Summarises a LabVIEW VI Server log into the sequence of calls a client made.

LabVIEW has **built-in VI Server call logging**. Enable it at runtime by writing
two Application properties, in this order:

1. `Server:Log File Path`  ← e.g. `C:\Git\lvpm\vi_server.log`
2. `Server:Logging Enabled` ← `TRUE`

It is **runtime-only**: there is no `LabVIEW.ini` token and no checkbox in
Tools » Options » VI Server, so it must be re-set every time LabVIEW starts.
(The ini-settable `WebServer.*` logging is a different, unrelated feature.)

```console
$ python tools/parse_viserver_log.py vi_server.log --gap 45
```

Caveat: LabVIEW logs only messages it **receives**. The `kTS*Return` replies are
never recorded, so the `err=0` on every request says nothing about whether the
operation succeeded.

## `deps.py`

Resolves a package's dependency closure offline from VIPM's cached `.spec`
manifests in `%ProgramData%\JKI\VIPM\cache`, and flags which packages declare
`[Script VIs]` hooks.

```console
$ python tools/deps.py bjustice_lib_rocketplot
```

## `deptree.py`

Reads the paths a VI declares in its linker tables (`LIvi`, `LIbd`, `LIfp`,
`LIds`) and audits them. Use this instead of hashing files, because **relinked
VIs are not byte-reproducible**: repeated relink passes over the same tree leave
~24 of 230 LUnit files with a different `LIfp` of *identical length* — LabVIEW
serialises the same entries in a different order each time. Measured over four
consecutive passes, the byte diff moved 12–19 files per pass while the
dependency sets moved **zero**.

```console
$ python tools/deptree.py show "<some.vi>"          # what it declares
$ python tools/deptree.py check <tree>              # do those paths resolve?
$ python tools/deptree.py compare <tree-a> <tree-b> # set-level diff
```

`check` is the correctness gate and needs no reference install: a declared path
that does not resolve is exactly what makes LabVIEW open its "searching for
missing subVI" dialog. It exits non-zero when any path fails. On
`astemes_lib_lunit` 2.0.13.6:

| tree | declared | unresolvable |
|---|---|---|
| as packaged | 188 | 19 |
| VIPM-relinked | 896 | **0** |

The 19 are stale absolutes left over from the package's own build machine
(`c/actions-runner/_work/astemes-lunit/...`). Relinking's real job is to replace
those with `<vilib>`-token paths, and to declare the dependencies the packaged
VI never recorded — packaged VIs declare ~0.9 paths each, relinked ones ~4.4.

## What the captures showed

Two installs were captured: `lunit` via the VIPM CLI, and
`bjustice_lib_rocketplot` 2.0.2.16 + dependencies (15 in the closure, 9 actually
installed) via the VIPM Desktop app.

**A package install is two phases.** VIPM copies *every* package's files to disk
first, then relinks each package in a separate cycle, then refreshes caches once:

| Phase | RocketPlot timing | VI Server traffic |
|---|---|---|
| File copy, all 9 packages, leaves-first | 76 s | none |
| Relink, one cycle per package | 377 s (**83 % of wall clock**) | 9 identical cycles |
| `Refresh Palettes` + `Refresh Menus 2013` | 10 s | once, at the very end |

**The relink cycle** is entirely `Call By Reference` into JKI's own
`callbacks.llb`, and amounts to a hand-rolled mass compile:

```
Set LLB Caching  ->  Open VI Reference [Multiple]   (load: recompiles to the
                                                     current LabVIEW version and
                                                     resolves subVI/typedef links)
                 ->  App Library Open [Multiple]    (resolves .lvlib member paths)
                 ->  VI Save Instrument [Multiple]  (persists both)
                 ->  Library Save [Multiple]        (persists member paths)
                 ->  Close VI / Close Library Reference [Multiple]
                 ->  Set LLB Caching
```

Recompile and relink are not separate steps — the **save** is what makes either
stick. Cost tracks package size: a 3.4 KB path list took 10 s to load; RocketPlot's
40.2 KB list took 98 s to load and 33 s to save.

**VIPM never calls LabVIEW's Mass Compile.** Confirmed on both captures —
`kTSAppDoMethodSend` never appears — despite
`Mass Compile After Package Install?="TRUE"` in its own `Settings.ini`.

**It does no cross-package relinking.** Only the newly installed packages are
touched; already-installed packages that link against them are left alone. And
because each cycle ends by closing every reference, shared dependencies (OpenG,
IlluminatedG) are reloaded from disk once per dependent package.

**The protocol surface is small** — a whole install needs 10 message types and
4 method IDs (1003 Run VI, 1051/1052/1053 Set/Get Control Value), against the
61 `kTS*` symbols in `LabVIEW.exe`. None of it requires reimplementing the wire
protocol: every operation maps onto the public VI Server API, reachable through
`LabVIEWCLI -PortNumber` or g-cli.

**Post-install hooks** (`[Script VIs]`) run as: extract to `%TEMP%\PostInstall.vi`
→ Open VI Reference → Set Control Value → Run VI → Get All Control Values.
Not yet captured in isolation — the only hooked package in the RocketPlot closure
(`jdp_science_jsontext`) was already installed, so its hook never fired.

## Implications for lvpm

- Copy all files first, then relink. Ordering then cannot break link resolution.
- Use `LabVIEWCLI -OperationName MassCompile`, which does load → recompile →
  resolve → save in one supported, headless call.
- `-NumberOfVIsToCache` defaults to **0**, meaning nothing stays resident. Raising
  it lets shared dependencies stay in memory across a compile — something VIPM's
  architecture structurally cannot do, since it closes all references between
  packages.
- Palette and menu refresh is still needed, once, at the end. Mass compile does
  not do it.
let
## `audit.py`

Static venv-readiness check straight from the cached `.vip` archives — no
install, no LabVIEW. Per package it counts VIs with embedded compiled code
(`VICD` block present, i.e. separate compiled code off) and classifies every
linker path as `<vilib>`-style token, missing token, drive-letter absolute, or
relative. Measured 2026-09-17: unrelinked packages load and run in a venv, so
these two properties are what decides whether the LabVIEW relink pass can be
skipped.

```console
$ python tools/audit.py oglib_error jki_lib_caraya
$ python tools/audit.py --vipc required-labview-packages.vipc --show absolute
```

LLBs are expanded: an `.llb` is an RSRC container with its member files laid
out verbatim inside, so members are audited as `x.llb/member.vi`.
