# lvpm

An open-source package manager for LabVIEW packages.

Resolves `.vip` / `.ogp` packages by name from the public repositories or a
local folder, downloads them, verifies the MD5, unpacks them into a real
LabVIEW installation, a scratch tree, or a project's own venv — then relinks
the installed VIs and runs the packages' install hooks through a running
LabVIEW's VI Server.

```console
$ lvpm targets
LabVIEW 2026 (64-bit)  v26.3       C:\Program Files\National Instruments\LabVIEW 2026
LabVIEW 2025 (64-bit)  v25.3       C:\Program Files\National Instruments\LabVIEW 2025
LabVIEW 2015 (32-bit)  v15         C:\Program Files (x86)\National Instruments\LabVIEW 2015

$ lvpm install --manifest lvpm.toml --labview-version 2026 --repo C:\my\packages
resolved 48 packages:
  delacor_lib_qmh 7.1.2.1547
  ...
+ delacor_lib_qmh 7.1.2.1547 ... [pre-install ok, 0.8s] 65 files
...
done. 8766 files written.

relinking 82 folder(s) with Relink Package.vi  (27 covered by a parent)
...
running 15 post-install hook(s)
  delacor_lib_qmh ... ok (0.4s)
```

## Why

LabVIEW packaging was open before it was closed. The **OpenG Package Tools**
(2002–2005, LGPL) designed and documented all of it: the zip-with-a-`spec`
package, the INI package directory, `Target Dir` tokens, file groups, replace
modes, install-time script VIs, the installed-package database, dependency and
conflict resolution, verification, and a per-project "development system
configuration" you could export and re-import. VIPM descends from that design
and is closed source; the design itself, its documents and its source are
still public at <https://ogpm.sourceforge.net/>.

lvpm goes back to that model and finishes it: same formats, so today's `.vip`
and `.ogp` packages and the public `.ogpd` / `.vipr` directories work as-is,
with the LabVIEW-side work — fixing VI links, running hook VIs — done over VI
Server, a documented TCP protocol. [docs/ogpm-model.md](docs/ogpm-model.md) is
the reference and the term-by-term map from OGPT to lvpm.

## What works

### Installing

- `lvpm install <name>[@<version>]` — resolve transitively, download, verify
  MD5, unpack
- `lvpm install --manifest <lvpm.toml>` — install every dependency a project
  manifest lists, as one plan with one relink pass
- `lvpm uninstall <name>` — remove exactly the files that were installed,
  prune empty dirs
- `lvpm list`, `lvpm search <query>`, `lvpm targets`
- `--dry-run` — show every file that would be written, every folder that
  would be relinked and every hook that would run, change nothing
- `--prefix <DIR>` — install into a sandbox instead of LabVIEW (file copy
  only; a scratch tree has no LabVIEW to relink or run hooks with)
- `--repo <URL-or-DIR>` — add a repository: a hosted `index.vipr` folder, or
  a plain local directory of `.vip` files, indexed straight from each
  package's own `spec`
- `--global` — install into the LabVIEW installation even from inside a
  project that has a venv (see below)
- `--no-relink` — copy the files but skip the relink pass; the default on a
  development machine is to relink, the CI case for skipping it is under
  [CI in a container](#ci-in-a-container-docker)

### The project manifest

`lvpm.toml` at the repo root is what a project depends on, and where from.
Any `lvpm` command run at or below it picks it up — its `[sources]` apply to
`search` and to a single-package `install` as much as to a full one:

```toml
[project]
name = "NovaXC"
version = "0.1.0"
labview = "2025"                  # the oldest LabVIEW the project is meant for

[sources]                         # repositories beyond the public indexes
local = "./Dependencies"          # a folder of .vip files, relative to this file
mirror = "http://host:8090/files" # or a hosted index.vipr folder
defaults = true                   # false: only the sources listed here

[dependencies]
delacor_lib_qmh = "7.1.2.1547"    # exactly this version
oglib_error = ">=6.0.1"           # the newest version satisfying the floor
jki_lib_caraya = "*"              # the newest version

[nipm.dependencies]               # NI Package Manager packages: reported, not installed
ni-daqmx-labview-support = "26.0"
```

`labview` is a minimum, not a pin: a venv binds to that version or a newer
one. Dependencies a package carries are always floors, so what a project pins
exactly stays exact and the rest resolves to the newest version that fits.

### Per-project dependencies (venvs)

A project can keep its packages to itself, the way a Python virtualenv or
`node_modules` does, instead of sharing one `vi.lib` with every other project
on the machine:

```console
$ cd C:\Git\MyProject
$ lvpm venv create --labview-version 2026
created C:\Git\MyProject\.project
  bound to       LabVIEW 2026 (64-bit)  v26.3
  VI Server port 3735

$ lvpm install                    # everything lvpm.toml lists, into .project
$ lvpm install oglib_error        # one more package, into .project
$ lvpm launch MyProject.lvproj    # a LabVIEW that sees the venv
```

- `lvpm venv create` makes `.project/` beside `lvpm.toml` and binds it to a
  LabVIEW version: `--labview-version`, else the manifest's `labview`, which
  is the project's *minimum* — binding to a newer IDE is fine, an older one is
  refused. `.project/` ignores itself in git; the manifest is what you commit.
  `lvpm venv status` / `lvpm venv remove` round it out.
- No activation step. Like cargo or npm, any `lvpm` command run at or below a
  directory holding a venv uses that venv; `--project <DIR>` names one from
  outside, `--global` says the installation itself is meant. An `lvpm.toml`
  with no venv beside it is an error, never a silent global install. Every
  venv-mode command prints which venv it resolved to.
- `lvpm launch [<lvproj>]` starts a LabVIEW on the venv: a copy of the
  target's `LabVIEW.ini` with the venv mounted and VI Server on a port of its
  own, handed over with `-pref`. Your primary LabVIEW, if open, is untouched.

How it works: `.project/` is an [LVAddons](https://labviewwiki.org/wiki/LVAddons)
location, one addon per package, each mirroring the LabVIEW install dir
under `<pkg>/1/`. LabVIEW started with `LVAddons.AdditionalLocations`
pointing there overlays them onto its own tree, so a package in the venv
resolves as `<vilib>/...` exactly as a globally installed one would — the
VIs you write against it link portably — while `C:\Program Files` never
changes. Needs LabVIEW 2024 Q1 or later.

Two things follow from LabVIEW reading an addon's contents at launch:

- After `lvpm install`, a LabVIEW that was already open on the venv cannot
  see the new files — restart it (`lvpm launch`). The relink runs inside a
  LabVIEW lvpm starts fresh on the venv for that reason, and is refused while
  one is already open there.
- The real installation always wins over the overlay: a package installed
  both globally and in the venv is loaded from the global copy, whatever its
  version. Keep the target install clean of what the venv provides.

### Relinking

Freshly unpacked VIs still declare the paths their build machine wrote.
`lvpm install` therefore ends with a relink pass: it drives
`tools/Relink Package.vi` over every installed package folder through
VI Server — load, let LabVIEW resolve the links, save — and prints each
folder's report of the files it actually rewrote.

- one pass over the whole plan, so no package is relinked before a later
  package's files are on disk
- overlapping folders are collapsed first: a folder nested inside another
  package's folder joins that walk instead of repeating it
- `lvpm relink <name>` / `lvpm relink --all` — rerun the pass from the
  install manifests, without reinstalling
- transient VI Server refusals (error 1000: the reference invalidated by the
  relink's own saves, or the VI not yet out of its running state) retry with
  a fresh reference and backoff

### Script-VI hooks

The install hooks packages declare are extracted from the archive and run at
their proper moments, each handed the action-info variant hook VIs read their
parameters from (`Package Name`, `LabVIEW Target Version`,
`Quiet Mode = TRUE`, `Files Installed`, ...), so they run headless instead of
parking dialogs over the install:

| Hook | Moment |
|---|---|
| `PreInstall.vi` | before the package's files are copied |
| `PostInstall.vi` | after the relink pass |
| `PreUninstall.vi` | before the package's files are removed |
| `PostUninstall.vi` | after removal (uninstall hooks are extracted at install time — the archive is long gone when uninstall needs them) |

Hook failures are reported, never fatal: the files are installed either way,
and an uninstall is never left half-done.

### VI Server primitives

The transport underneath relink and hooks is a typed VI Server TCP client
(`viserver.rs`) — no LabVIEWCLI, no ActiveX. It speaks the flattened-data
formats: scalars, strings, paths, n-dimensional arrays, and variants with
named attributes. Documented in
[docs/vi-server-protocol.md](docs/vi-server-protocol.md); exercised directly
with:

- `lvpm vi-probe <VI>` — open a reference and release it, proving the
  transport
- `lvpm vi-run <VI> --set name=type:value --get name` — set controls, run,
  read values back (types `bool|i32|dbl|str`, plus `type[]:` arrays)
- `lvpm vi-save <VI>` — load and save one VI, the relink primitive

Relinking and hooks need a running LabVIEW with VI Server (TCP) enabled;
everything else runs without any LabVIEW process. The bundled
`Relink Package.vi` is saved for LabVIEW 2020, so any LabVIEW from 2020 on
can load it.

Verified end to end: a 46-dependency project manifest resolved to 48
packages / 8766 files, installed, relinked in one deduplicated pass and its
15 post-install hooks run, from a single `lvpm install --manifest`.

### CI in a container (Docker)

lvpm runs unchanged in NI's Windows container image
(`nationalinstruments/labview:latest-windows`, LabVIEW 2026 and later): the
image carries a real LabVIEW install, the registry keys `lvpm targets` reads,
and a `LabVIEW.ini` with VI Server already enabled on port 3363. A CI job is
the project's own manifest installed globally, then the LabVIEWCLI operation
you want:

```dockerfile
FROM nationalinstruments/labview:latest-windows
ENV LV_RTE_HEADLESS=1
COPY lvpm.exe C:/lvpm/
COPY . C:/src
RUN C:\lvpm\lvpm.exe install --manifest C:\src\lvpm.toml --global --labview-version 2026 --no-relink
RUN LabVIEWCLI -OperationName MassCompile -DirectoryToCompile C:\src -Headless
```

Three things make that work:

- **`LV_RTE_HEADLESS=1`.** A container has no desktop, so a LabVIEW started
  normally blocks forever on an activation prompt nobody can see. That
  variable makes every LabVIEW start headless: no activation, no dialogs,
  errors to `%TEMP%\LabVIEW_*_headless_*_cur.txt` instead. lvpm needs no
  flag of its own — the LabVIEW it starts for hooks inherits the variable.
  When it is set, lvpm also skips the palette and menu refresh, since a
  headless LabVIEW shows a palette to no one (`lvpm refresh` still works).
- **`--global`.** Only needed when an `lvpm.toml` sits at or above the
  working directory: lvpm then refuses to install into the machine unless
  told to, so a developer inside a project cannot do it by accident (see
  venvs below). `--manifest <path>` alone does not trigger that check, so
  with the default working directory the flag is redundant here — it is in
  the example so a `WORKDIR C:/src` does not turn the install into a
  refusal. In a throwaway container the machine *is* the sandbox.
- **`--no-relink`.** Relinking stays the default on a development machine,
  where unrelinked package VIs load fine but show up as modified and want
  saving. Nothing in a container opens the IDE, and a mass compile or build
  resolves the links itself as it loads — measured on a DQMH project: the
  compile succeeded with zero bad VIs against an unrelinked DQMH install,
  and skipping the pass saved about ten minutes. Leave the flag off if the
  job is to *produce* relinked packages rather than consume them.

Two container facts worth knowing. LabVIEW allows one mode at a time per
machine, headless or IDE, so a headless job cannot share a host with an open
IDE. And a LabVIEW that lvpm started outlives lvpm by design; in a container
that keeps the container alive, so end a job with
`LabVIEWCLI -OperationName CloseLabVIEW -Headless` (a `RUN` step exits on
its own — this matters for `docker run`).

NI's container license permits CI/CD, automated tests, mass compiles and
builds, and forbids editing LabVIEW code in a container; whether a relink
pass in CI counts as the latter is your reading of those terms, not lvpm's.

## Package sources

Two public, anonymous, plain-HTTP package directories, which between them
cover approximately 98% of the packages listed on vipm.io:

| Source | Packages |
|---|---|
| `download.ni.com/evaluation/labview/lvtn/vipm/index.vipr` | ~2,530 |
| `www.jkisoft.com/packages/jkisoft.ogpd` | ~2,290 |

Both are OGPM's Package Directory format — `[Package <name>-<version>]`
sections with a `Package.URL` — unchanged since `openg.ogpd` in 2004; `.vipr`
adds an MD5 per entry. Anything they lack can come from a folder of `.vip` /
`.ogp` files via `--repo`, OGPM's "local repository": a directory of named
packages is its own index. A project's own `Dependencies` directory works
as-is.

## Known limitations

- `Replace Mode = If Newer` currently only writes when the file is absent;
  doing it properly means comparing archive timestamps against disk.
- No dependency conflict resolution — the first resolution of a name wins,
  and a floor resolves to the newest version satisfying it.
- No mass compile; LabVIEW recompiles installed VIs on first load.
- Hook VIs' own `error out` is not read back — a hook that fails internally
  but runs to completion reports as ok.
- A failure part-way through unpacking leaves files behind with no manifest,
  and one failed download aborts the rest of the plan.
- Windows is the tested platform; Linux target detection exists but is
  untested.
- In a venv, install hooks are extracted but never run (they act on the
  LabVIEW installation, not on an overlay), and file groups aimed at machine
  locations (`<OS ...>`, `<temp>`) are skipped — both are recorded in the
  manifest and shown by `lvpm list`. `lvpm install <name>` in a venv does not
  add the pin to `lvpm.toml`. No warning yet when a global copy of a venv
  package shadows it. See [docs/roadmap.md](docs/roadmap.md) for what is
  planned.

## Build

```console
cargo build --release
cargo test
```

## Layout

| Module | Responsibility |
|---|---|
| `index.rs` | Fetch and parse package directories (`.ogpd` / `index.vipr`), resolve names to versions, scan local repositories |
| `spec.rs` | The `spec` manifest inside a package (the OGPT `.ogp` format and VIPM's `.vip` dialect) |
| `project.rs` | The project manifest `lvpm.toml`: dependencies and their constraints, sources, minimum LabVIEW, lookup from a directory |
| `target.rs` | LabVIEW detection, and where each `Target Dir` token points |
| `install.rs` | Plan, unpack, extract hook VIs, record a manifest, uninstall |
| `venv.rs` | A project's `.project/` venv: binding, lookup from the working directory, `lvpm venv` |
| `launch.rs` | Start LabVIEW on a venv: the `-pref` ini, readiness by handshake, detached spawning |
| `relink.rs` | Drive the relink VI over installed folders; folder collapsing and retries |
| `refresh.rs` | Rebuild palettes and menus through LabVIEW's own shipping VIs |
| `viserver.rs` | The VI Server TCP protocol: connection, methods, flattened data |
| `version.rs` | Package version ordering: OGPT `version-release`, VIPM's four-part form; not semver |

## Status

Working, in active development. Not affiliated with JKI or NI.

## License

MIT OR Apache-2.0
