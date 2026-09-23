# lvpm

An open-source package manager for LabVIEW.

lvpm resolves `.vip` / `.ogp` packages by name from the public package
directories or a local folder, downloads them, verifies the MD5 and unpacks
them into a LabVIEW installation or a project's own venv.
It then relinks the installed VIs and runs the packages' install hooks
through LabVIEW's VI Server.

```console
$ lvpm targets
LabVIEW 2026 (64-bit)  v26.3       C:\Program Files\National Instruments\LabVIEW 2026
LabVIEW 2025 (64-bit)  v25.3       C:\Program Files\National Instruments\LabVIEW 2025

$ lvpm install --manifest lvpm.toml --labview-version 2026
resolved 48 packages:
  delacor_lib_qmh 7.1.2.1547
  ...
+ delacor_lib_qmh 7.1.2.1547 ... [pre-install ok, 0.8s] 65 files
...
done. 8766 files written.

relinking 82 folder(s)  (27 covered by a parent)
...
running 15 post-install hook(s)
  delacor_lib_qmh ... ok (0.4s)
```

Not affiliated with JKI or NI. Working, in active development; see
[docs/roadmap.md](docs/roadmap.md) and [Known limitations](#known-limitations).

## Install lvpm

Windows 10/11 with LabVIEW 2020 or later. Linux target detection exists but
is untested.

### With the install script

[scripts/install.ps1](scripts/install.ps1) fetches the latest release,
verifies the zip's SHA-256 against the digest GitHub records for it (and
against a `SHA256SUMS` file when a release ships one), extracts `lvpm.exe`
to `%LOCALAPPDATA%\lvpm\bin`, removes Windows' mark-of-the-web from the
files and adds the folder to your user `PATH`. It runs nothing it downloaded
except `lvpm --version` at the end. Read it first; it is short.

```powershell
irm https://raw.githubusercontent.com/Flydroid/lvpm/main/scripts/install.ps1 -OutFile install.ps1
.\install.ps1
```

Options: `-Version v0.1.0-alpha.1` pins a release, `-InstallDir <DIR>` and
`-NoPath` control where it goes, `-Sha256 <hash>` adds a hash you obtained
elsewhere as a further check, `-Stable` skips pre-releases. If PowerShell
refuses to run scripts:
`powershell -ExecutionPolicy Bypass -File .\install.ps1`.

### By hand

Download `lvpm-<version>-windows-x64.zip` from the
[Releases](https://github.com/Flydroid/lvpm/releases) page, right-click it,
tick **Unblock**, extract, and put the folder on your `PATH`. 


### From source

```console
cargo build --release      # target\release\lvpm.exe
cargo test
```

Toolchain setup is in [CONTRIBUTING.md](CONTRIBUTING.md).

## Quick start

```console
$ lvpm targets                              # which LabVIEW installations lvpm sees
$ lvpm search error                         # find packages in the public indexes
$ lvpm install oglib_error --labview-version 2026
$ lvpm list                                 # what lvpm has installed there
$ lvpm uninstall oglib_error
```

`--labview-version` picks the target when more than one LabVIEW is
installed. Relinking and hooks need LabVIEW's VI Server (TCP) enabled, which
is its default setting (Tools » Options » VI Server). lvpm starts LabVIEW
itself when it is not running, and says so.

For a project, write an `lvpm.toml` (next section), then:

```console
$ cd C:\Git\MyProject
$ lvpm venv create --labview-version 2026   # .project/ beside lvpm.toml
$ lvpm install                              # everything lvpm.toml lists, into .project
$ lvpm launch MyProject.lvproj              # a LabVIEW that sees the venv
```

## Why

LabVIEW packaging was open before it was closed. The **OpenG Package Tools**
(2002–2005) designed and documented all of it: the zip-with-a-`spec`
package, the INI package directory, `Target Dir` tokens, file groups, replace
modes, install-time script VIs, the installed-package database, dependency
resolution, and a per-project "development system configuration" you could
export and re-import. VIPM descends from that design and is closed source;
the design, its documents and its source are still public at
<https://ogpm.sourceforge.net/>.

lvpm goes back to that model and finishes it: same formats, so today's `.vip`
and `.ogp` packages and the public `.ogpd` / `.vipr` directories work as-is,
with the LabVIEW-side work (fixing VI links, running hook VIs) done over VI
Server, a documented TCP protocol. [docs/ogpm-model.md](docs/ogpm-model.md)
is the reference and the term-by-term map from OGPT to lvpm.

## Commands

| Command | What it does |
|---|---|
| `lvpm install <name>[@<version>]` | Resolve the package and its dependencies, download, verify MD5, unpack, relink, run hooks |
| `lvpm install` | Inside a project: install everything `lvpm.toml` lists, as one plan with one relink pass (`--manifest <FILE>` names the manifest explicitly) |
| `lvpm uninstall <name>` / `--all` | Remove exactly the files that were installed and prune empty folders |
| `lvpm list` | What lvpm has installed into the selected target, and which hooks it did not run |
| `lvpm search <query>` | Search the package indexes |
| `lvpm targets` | Detected LabVIEW installations |
| `lvpm relink <name>` / `--all` | Rerun the relink pass from the install manifests, without reinstalling |
| `lvpm run-hooks <name>` / `--all` | Rerun a package's PostInstall hook, for one that failed during install |
| `lvpm refresh` | Rebuild palettes and the File/Tools/Help menus from disk (`install` does this itself) |
| `lvpm start` | Start the selected LabVIEW and wait until its VI Server answers |
| `lvpm venv create` / `status` / `remove` | Manage the project's venv (see [Per-project dependencies](#per-project-dependencies-venvs)) |
| `lvpm launch [<lvproj>]` | Start a LabVIEW on the project's venv |
| `lvpm vi-probe` / `vi-run` / `vi-save` / `app-probe` | VI Server primitives, for diagnosis (see [VI Server](#vi-server)) |

Flags that apply to every command:

| Flag | Meaning |
|---|---|
| `--labview-version <YYYY>` | Install into this LabVIEW version (see `lvpm targets`) |
| `--project <DIR>` | Use that project's venv instead of looking upwards from the current directory |
| `--global` | Use the LabVIEW installation itself, even from inside a project that has a venv |
| `--prefix <DIR>` | Install into a scratch directory instead of LabVIEW (file copy only; nothing to relink or run hooks with) |
| `--repo <URL-or-DIR>` | Add a repository: a hosted `index.vipr` folder, or a plain local directory of `.vip` files. Repeatable |
| `--refresh` | Re-download the indexes instead of using the cache in `%LOCALAPPDATA%\lvpm\cache` |

`install` flags: `--dry-run` shows every file, relink folder and hook and
changes nothing; `--no-deps` skips dependencies; `--allow-downgrade` permits
replacing an installed package with an older version; `--no-relink` skips the
relink pass and `--relink` forces it where it is off by default; `--hooks`
runs install hooks where they are off by default (both: a headless LabVIEW,
see [CI in a container](#ci-in-a-container-docker)). `--relink-timeout <SECS>`
(default 900) bounds one folder's relink; `--keep-open` leaves the LabVIEW
started for a venv relink running.

## The project manifest

`lvpm.toml` at the repo root says what a project depends on, and where from.
Any `lvpm` command run at or below it picks it up; its `[sources]` apply to
`search` and to a single-package `install` as much as to a full one.

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

## Per-project dependencies (venvs)

A project can keep its packages to itself, the way a Python virtualenv or
`node_modules` does, instead of sharing one `vi.lib` with every other project
on the machine.

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
  LabVIEW version: `--labview-version`, else the manifest's `labview`.
  Binding to a newer IDE than the manifest's minimum is fine; an older one is
  refused. `.project/` ignores itself in git; the manifest is what you
  commit. `lvpm venv status` shows the binding, `lvpm venv remove` deletes
  the venv and everything in it.
- No activation step. Like cargo or npm, any `lvpm` command run at or below a
  directory holding a venv uses that venv. `--project <DIR>` names one from
  outside; `--global` says the installation itself is meant. An `lvpm.toml`
  with no venv beside it is an error, never a silent global install, except
  on a headless machine (see [CI in a container](#ci-in-a-container-docker)).
  Every venv-mode command prints which venv it resolved to.
- `lvpm launch [<lvproj>]` starts a LabVIEW on the venv: a copy of the
  target's `LabVIEW.ini` with the venv mounted and VI Server on a port of its
  own, handed over with `-pref`. Your primary LabVIEW, if open, is untouched.

How it works: `.project/` is an [LVAddons](https://labviewwiki.org/wiki/LVAddons)
location, one addon per package, each mirroring the LabVIEW install dir
under `<pkg>/1/`. A LabVIEW started with `LVAddons.AdditionalLocations`
pointing there overlays them onto its own tree, so a package in the venv
resolves as `<vilib>/...` exactly as a globally installed one would. The VIs
you write against it link portably, and `C:\Program Files` never changes.
Needs LabVIEW 2024 Q1 or later.

Two things follow from LabVIEW reading an addon's contents at launch:

- After `lvpm install`, a LabVIEW already open on the venv cannot see the new
  files. Restart it with `lvpm launch`. For the same reason the relink runs
  in a LabVIEW that lvpm starts fresh on the venv, and is refused while one
  is already open there.
- The real installation always wins over the overlay: a package installed
  both globally and in the venv loads from the global copy, whatever its
  version. Keep the target installation clean of what the venv provides.

## What an install does in LabVIEW

### Relinking

Freshly unpacked VIs still declare the paths their build machine wrote.
`lvpm install` therefore ends with a relink pass: a small relink VI, embedded
in `lvpm.exe` and written to `%TEMP%` when needed, is driven over every
installed package folder through VI Server. It loads each VI, lets LabVIEW
resolve the links, saves, and reports the files it actually rewrote. The VI
is saved for LabVIEW 2020, so any LabVIEW from 2020 on can load it.

- One pass over the whole plan, so no package is relinked before a later
  package's files are on disk.
- Overlapping folders are collapsed first: a folder nested inside another
  package's folder joins that walk instead of repeating it.
- Transient VI Server refusals (error 1000: a reference invalidated by the
  relink's own saves, or a VI not yet out of its running state) retry with a
  fresh reference and backoff.
- `lvpm relink <name>` / `lvpm relink --all` reruns the pass from the
  install manifests, without reinstalling.

### Script-VI hooks

The install hooks a package declares are extracted from the archive and run
at their proper moments. Each is handed the action-info variant hook VIs
read their parameters from (`Package Name`, `LabVIEW Target Version`,
`Quiet Mode = TRUE`, `Files Installed`, ...), so they run without parking
dialogs over the install.

| Hook | Moment |
|---|---|
| `PreInstall.vi` | before the package's files are copied |
| `PostInstall.vi` | after the relink pass |
| `PreUninstall.vi` | before the package's files are removed |
| `PostUninstall.vi` | after removal (uninstall hooks are extracted at install time; the archive is long gone when uninstall needs them) |

Hook failures are reported, never fatal: the files are installed either way,
and an uninstall is never left half-done. `lvpm run-hooks <name>` reruns a
PostInstall that failed. After the hooks, `install` rebuilds the palettes and
menus so a package's palette and Tools entries appear without restarting
LabVIEW; `lvpm refresh` does that step alone.

### VI Server

The transport underneath relink and hooks is a typed VI Server TCP client
(`viserver.rs`): no LabVIEWCLI, no ActiveX. It speaks the flattened-data
formats (scalars, strings, paths, n-dimensional arrays, and variants with
named attributes) and is documented in
[docs/vi-server-protocol.md](docs/vi-server-protocol.md). lvpm reads the
target's port from its `LabVIEW.ini` (LabVIEW 2025 defaults to 3363, 2026 to
3364) and starts the IDE when nothing answers there. Only relinking and hooks
need LabVIEW; everything else runs without any LabVIEW process.

For diagnosis the primitives are exposed directly:

- `lvpm vi-probe <VI>` opens a reference and releases it, proving the
  transport.
- `lvpm vi-run <VI> --set name=type:value --get name` sets controls, runs,
  and reads values back (types `bool|i32|dbl|str`, plus `type[]:` arrays).
- `lvpm vi-save <VI>` loads and saves one VI, the relink primitive.
- `lvpm app-probe <ID>...` invokes parameterless Application methods by raw
  id. It invokes whatever it hits; sweep with care.

Verified end to end: a 46-dependency project manifest resolved to 48
packages / 8766 files, installed, relinked in one deduplicated pass and its
15 post-install hooks run, from a single `lvpm install --manifest`.

## CI in a container (Docker)

lvpm runs unchanged in NI's Windows container image
(`nationalinstruments/labview:latest-windows`, LabVIEW 2026 and later). The
image carries a real LabVIEW install, the registry keys `lvpm targets`
reads, and a `LabVIEW.ini` with VI Server already enabled. The command is
the same one a developer runs in the project, `lvpm install`, followed by
whatever LabVIEWCLI operation the job is for:

```dockerfile
FROM nationalinstruments/labview:latest-windows
ENV LV_RTE_HEADLESS=1
COPY lvpm.exe C:/lvpm/
COPY . C:/src
WORKDIR C:/src
RUN C:\lvpm\lvpm.exe install
RUN LabVIEW.exe "C:\Program Files\National Instruments\LabVIEW 2026\vi.lib\addons\_JKI Toolkits\Caraya\Caraya CLI.vi" -- -s C:\src\Tests -x C:\out\junit.xml
RUN LabVIEWCLI -OperationName MassCompile -DirectoryToCompile C:\src -Headless
```

Measured: the install takes 11 seconds and starts no LabVIEW. Caraya's CLI
then gets a LabVIEW of its own, runs every test under `Tests`, writes JUnit
and quits (121 cases of Caraya's own suite in 90 s). The mass compile takes
47 s.

`LV_RTE_HEADLESS=1` is NI's own switch, not lvpm's. It makes every LabVIEW
start headless: no activation, no dialogs, errors to
`%TEMP%\LabVIEW_*_headless_*_cur.txt` instead. Without it a LabVIEW started
in a container blocks forever on an activation prompt nobody can see. Nobody
sets it on a workstation, so its presence is a reliable sign of an
automation machine with no developer at it, and lvpm reads it as exactly
that. On a headless machine three decisions go the other way, each announced
on one line and each overridable:

| | developer's machine | headless (`LV_RTE_HEADLESS`) | override |
|---|---|---|---|
| `lvpm.toml` with no venv beside it | refused: a developer inside a project must not install into the machine by accident | the machine is the sandbox: install into the LabVIEW the manifest's `labview` names (a newer one may stand in, an older one may not) | `--labview-version`, `--project` |
| relink pass | on: unrelinked package VIs load, but show as modified and want saving | off: nothing opens the IDE, and the compile or build resolves the links as it loads | `--relink` |
| install hooks | run, before and after the files | skipped, and `lvpm list` says so: the hooks measured write a marker file and add a palette entry, IDE furniture | `--hooks` |
| palettes and menus | rebuilt after the install | not rebuilt: no one sees a palette | `lvpm refresh` |

Together those mean a headless `lvpm install` never starts LabVIEW: it is
downloads, MD5 checks and file copies, and takes seconds. Measured on a DQMH
project: two minutes with hooks, twelve with relink as well, and the mass
compile afterwards reported nothing wrong either way.

The hook skip is the one with a caveat. A hook is arbitrary LabVIEW code,
and a few packages do real setup in theirs: NI's HTTP client, for one, does
not open a session until its PostInstall has run. If a package behaves
differently in the container than on your machine, `--hooks` is the first
thing to try; it starts a headless LabVIEW for them. `--relink` likewise
when the job's output *is* relinked packages.

Two container facts worth knowing. LabVIEW allows one mode at a time per
machine, headless or IDE, so a headless job cannot share a host with an open
IDE. And any LabVIEW that is started (by `--hooks`, `--relink`, or the job
itself) outlives lvpm by design; in a container that keeps a `docker run`
alive, so end such a job with
`LabVIEWCLI -OperationName CloseLabVIEW -Headless` (a Dockerfile `RUN` step
exits on its own). Tools that want a LabVIEW of their own, such as Caraya's
CLI, need none to be running first, which the plain headless install
guarantees.

## Package sources

Two public, anonymous, plain-HTTP package directories, which between them
cover roughly 98% of the packages listed on vipm.io:

| Source | Packages |
|---|---|
| `download.ni.com/evaluation/labview/lvtn/vipm/index.vipr` | ~2,530 |
| `www.jkisoft.com/packages/jkisoft.ogpd` | ~2,290 |

Both are OGPM's Package Directory format, `[Package <name>-<version>]`
sections with a `Package.URL`, unchanged since `openg.ogpd` in 2004; `.vipr`
adds an MD5 per entry. Anything they lack can come from a folder of `.vip` /
`.ogp` files via `--repo` or the manifest's `[sources]`: a directory of named
packages is its own index, so a project's `Dependencies` folder works as-is.
Downloaded indexes are cached in `%LOCALAPPDATA%\lvpm\cache`; `--refresh`
fetches them again.

## Known limitations

- `Replace Mode = If Newer` only writes when the file is absent; doing it
  properly means comparing archive timestamps against disk.
- No dependency conflict resolution: the first resolution of a name wins,
  and a floor resolves to the newest version satisfying it.
- No mass compile; LabVIEW recompiles installed VIs on first load.
- A hook VI's own `error out` is not read back. A hook that fails internally
  but runs to completion reports as ok.
- A failure part-way through unpacking leaves files behind with no manifest,
  and one failed download aborts the rest of the plan.
- Windows is the tested platform.
- In a venv, install hooks are extracted but never run (they act on the
  LabVIEW installation, not on an overlay), and file groups aimed at machine
  locations (`<OS ...>`, `<temp>`) are skipped. Both are recorded in the
  manifest and shown by `lvpm list`. `lvpm install <name>` in a venv does not
  add the pin to `lvpm.toml`. No warning yet when a global copy of a venv
  package shadows it.

[docs/roadmap.md](docs/roadmap.md) has what is planned.

## Layout

| Module | Responsibility |
|---|---|
| `main.rs` | The CLI: commands, target and venv selection, the install plan |
| `index.rs` | Fetch and parse package directories (`.ogpd` / `index.vipr`), resolve names to versions, scan local repositories |
| `spec.rs` | The `spec` manifest inside a package (the OGPT `.ogp` format and VIPM's `.vip` dialect) |
| `project.rs` | The project manifest `lvpm.toml`: dependencies and their constraints, sources, minimum LabVIEW, lookup from a directory |
| `target.rs` | LabVIEW detection, and where each `Target Dir` token points |
| `install.rs` | Plan, unpack, extract hook VIs, record a manifest, uninstall |
| `venv.rs` | A project's `.project/` venv: binding, lookup from the working directory, `lvpm venv` |
| `launch.rs` | Start LabVIEW on a venv: the `-pref` ini, readiness by handshake, detached spawning |
| `relink.rs` | Drive the embedded relink VI over installed folders; folder collapsing and retries |
| `refresh.rs` | Rebuild palettes and menus through LabVIEW's own shipping VIs |
| `viserver.rs` | The VI Server TCP protocol: connection, methods, flattened data |
| `version.rs` | Package version ordering: OGPT `version-release`, VIPM's four-part form; not semver |
| `src/lv-src/relink-package.vi` | The relink VI embedded into the executable at build time (LabVIEW 2020) |
| `scripts/install.ps1` | Download, verify and install the latest release |

## License

MIT OR Apache-2.0
