# lvpm

An open-source package manager for LabVIEW.

lvpm resolves `.vip` / `.ogp` packages by name from the public package
directories, hosted indexes, or local package folders configured globally.
It downloads hosted packages, verifies the MD5 and unpacks them into a
LabVIEW installation or a project's own venv.
It then prepares the installed LabVIEW files with **linkfix**: repair packaged
dependency links and separate embedded compiled code, without starting LabVIEW.
Package install hooks run through LabVIEW's VI Server; a LabVIEW load/save
relink pass is available separately with `--relink`.

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

linkfix: preparing installed files (repair links and separate compiled code; no LabVIEW needed)
...
running 15 post-install hook(s)
  delacor_lib_qmh ... ok (0.4s)
```

Not affiliated with JKI or NI. Working, in active development; see
[docs/roadmap.md](docs/roadmap.md) and [Known limitations](#known-limitations).

## Install lvpm

Windows 10/11 with LabVIEW 2020 or later. On Linux, lvpm is verified against
LabVIEW 2026 Q3 in NI's container image; take `lvpm-<version>-linux-x64.tar.gz`
from the artifacts of a CI run, or build it from source, and see
[docs/linux.md](docs/linux.md) for what differs there.

### With the install script

[scripts/install.ps1](scripts/install.ps1) fetches the latest release,
verifies the zip's SHA-256 against the digest GitHub records for it (and
against a `SHA256SUMS` file when a release ships one), extracts `lvpm.exe`
to `%LOCALAPPDATA%\lvpm\bin`, removes Windows' mark-of-the-web from the
files and adds the folder to your user `PATH`. It runs nothing it downloaded
except `lvpm --version` at the end. Read it first; it is short.

```powershell
cd $env:TEMP
irm https://raw.githubusercontent.com/Flydroid/lvpm/main/scripts/install.ps1 -OutFile install.ps1
.\install.ps1
```

The `cd` matters: the script is written to the current directory, and an
elevated PowerShell starts in `C:\WINDOWS\system32`, which you cannot write
to. `Access to the path 'C:\WINDOWS\system32\install.ps1' is denied` means
you are in a directory that is not yours, not that anything is wrong with
the download. lvpm installs per-user and needs no elevation.

Options: `-Version v0.2.0` pins a release, `-InstallDir <DIR>` and
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

### Test a local release installation

From a Windows checkout with the build tools installed:

```powershell
.\scripts\install.ps1 -BuildLocal
```

This runs `cargo build --release --locked`, packages the executable,
README and licenses into a temporary zip, verifies its SHA-256, and uses
the normal release installation path. It replaces any existing per-user
lvpm in `%LOCALAPPDATA%\lvpm\bin` and adds that directory to your user
`PATH`. No GitHub release or cargo-dist setup is required. The version
remains the one declared in Cargo.toml; temporary packaging files are removed.
The local checksum checks the packaged bytes, not publisher authenticity.

For an isolated test without changing your installed lvpm or user PATH:

```powershell
.\scripts\install.ps1 -BuildLocal -InstallDir "$PWD\sandbox\local-installer\bin" -NoPath
.\sandbox\local-installer\bin\lvpm.exe --version
```

An existing release zip can also be installed with `-Archive <FILE>` and
`-Sha256 <EXPECTED-HASH>` instead of building it. Local modes cannot be
combined with GitHub release selection options.

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
$ lvpm venv create --labview-version 2026   # .lv-venv/ beside lvpm.toml by default
$ lvpm install                              # everything lvpm.toml lists, into .lv-venv
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
with link preparation in Rust and LabVIEW-side work (running hook VIs and
optional load/save relinking) over VI Server, a documented TCP protocol. [docs/ogpm-model.md](docs/ogpm-model.md)
is the reference and the term-by-term map from OGPT to lvpm.

## Commands

| Command                                              | What it does                                                                                                                                 |
| ---------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| `lvpm install <name>[@<version>]`                    | Resolve dependencies, download, verify MD5, unpack, prepare links and source-only files, run hooks |
| `lvpm install`                                       | Inside a project: install everything `lvpm.toml` lists, preparing links after the whole plan is copied (`--manifest <FILE>` selects a manifest) |
| `lvpm linkfix` / `--tree <DIR>`                     | Prepare an existing venv or explicit LabVIEW-shaped tree; supports dry runs and per-file reports |
| `lvpm uninstall <name>` / `--all`                    | Remove installed files; optionally remove unshared dependencies                                                                              |
| `lvpm list`                                          | What lvpm has installed into the selected target, and which hooks it did not run                                                             |
| `lvpm search <query>`                                | Search the package indexes                                                                                                                   |
| `lvpm targets`                                       | Detected LabVIEW installations                                                                                                               |
| `lvpm relink <name>` / `--all`                       | Rerun the relink pass from the install manifests, without reinstalling                                                                       |
| `lvpm run-hooks <name>` / `--all`                    | Rerun a package's PostInstall hook, for one that failed during install                                                                       |
| `lvpm refresh`                                       | Rebuild palettes and the File/Tools/Help menus from disk (`install` does this itself)                                                        |
| `lvpm start`                                         | Start the selected LabVIEW and wait until its VI Server answers                                                                              |
| `lvpm venv create` / `status` / `remove`             | Manage the project's venv (see [Per-project dependencies](#per-project-dependencies-venvs))                                                  |
| `lvpm launch [<lvproj>]`                             | Start a LabVIEW on the project's venv                                                                                                        |
| `lvpm vi-probe` / `vi-run` / `vi-save` / `app-probe` | VI Server primitives, for diagnosis (see [VI Server](#vi-server))                                                                            |

Flags that apply to every command:

| Flag                       | Meaning                                                                                                   |
| -------------------------- | --------------------------------------------------------------------------------------------------------- |
| `--labview-version <YYYY>` | Install into this LabVIEW version (see `lvpm targets`)                                                    |
| `--project <DIR>`          | Use that project's venv instead of looking upwards from the current directory                             |
| `--global`                 | Use the LabVIEW installation itself, even from inside a project that has a venv                           |
| `--prefix <DIR>`           | Install into a scratch directory instead of LabVIEW; linkfix still runs, but LabVIEW relink and hooks cannot |
| `--refresh`                | Ask each index whether it changed (a conditional request) instead of trusting the cache in `%LOCALAPPDATA%\lvpm\cache` |

`install` flags: `--dry-run` shows every file, relink folder and hook and
changes nothing; `--no-deps` skips dependencies; `--allow-downgrade` permits
replacing an installed package with an older version; `--no-linkfix` leaves
package files as copied, skipping link repair and source-only preparation.
`--relink` loads and saves VIs in LabVIEW **instead of linkfix** (off by default);
the legacy `--no-relink` is accepted but does not disable linkfix. `--hooks`
runs install hooks where they are off by default (both: a headless LabVIEW,
see [CI in a container](#ci-in-a-container-docker)). `--relink-timeout <SECS>`
(default 900) bounds one folder's relink; `--keep-open` leaves the LabVIEW
started for a venv relink running.

## The project manifest

`lvpm.toml` at the repo root says what a project depends on, the minimum
LabVIEW it targets, and which hosted indexes to use. See
[docs/manifest.md](docs/manifest.md) for the format, how it is found and
resolved, and compatibility with VIPM's `vipm.toml`.

## Per-project dependencies (venvs)

A project can keep its packages to itself, the way a Python virtualenv or
`node_modules` does, instead of sharing one `vi.lib` with every other project
on the machine.

```console
$ cd C:\Git\MyProject
$ lvpm venv create --labview-version 2026
created C:\Git\MyProject\.lv-venv
  bound to       LabVIEW 2026 (64-bit)  v26.3
  VI Server port 3735

$ lvpm install                    # everything lvpm.toml lists, into .lv-venv
$ lvpm install oglib_error        # one more package, into .lv-venv
$ lvpm launch MyProject.lvproj    # a LabVIEW that sees the venv
```

- `lvpm venv create` makes `.lv-venv/` beside `lvpm.toml` by default and binds it to a
  LabVIEW version: `--labview-version`, else the manifest's `labview`.
  Binding to a newer IDE than the manifest's minimum is fine; an older one is
  refused. The venv directory ignores itself in git; the manifest is what you
  commit. `lvpm venv status` shows the binding, `lvpm venv remove` deletes
  the venv and everything in it.
- No activation step. Like cargo or npm, any `lvpm` command run at or below a
  directory holding a venv uses that venv. `--project <DIR>` names one from
  outside; `--global` says the installation itself is meant. An `lvpm.toml`
  with no venv beside it is an error, never a silent global install, except
  on a headless machine (see [CI in a container](#ci-in-a-container-docker)).
  Every venv-mode command prints which venv it resolved to.
- `lvpm launch [<lvproj>]` starts a LabVIEW on the venv: an ini of the
  venv's own (`.lv-venv/labview.ini`, copied from the target's `LabVIEW.ini` by
  `lvpm venv create` without its recent-files lists, so the venv IDE never
  offers projects from outside the venv; options you set in the venv IDE
  persist; delete it
  to inherit your global settings again at the next launch) with the venv
  mounted, VI Server on a port of its
  own, and its own compiled-object cache, `LabVIEW Data` folder and temp
  directory under `.lv-venv/`, handed over with `-pref`. Your primary LabVIEW,
  if open, is untouched, and its autosave files, cache and log
  (`LabVIEW_*_cur.txt`) are never shared with the venv's — look for the
  venv's log in `.lv-venv/temp`. The venv starts with an empty `LabVIEW Data`, so personal
  customisations kept there (icon templates, right-click plugins) are not
  available in it.

The default layout is:

```text
.lv-venv/
  packages/<pkg>/1/    package addon and its mirrored LabVIEW tree
  installed/           package installation records
  venv.json            LabVIEW binding
  labview.ini          venv-specific LabVIEW settings
  logs/lvpm.log        lvpm's log of every command run on this venv
  objcache/ data/ temp/  isolated LabVIEW runtime files
```

lvpm keeps one continuous log per venv in `.lv-venv/logs/lvpm.log`. Each command
appends a header line (the command and its directory), everything it printed,
detail it does not print, and how it ended (`---- done` or `---- failed: …`).
Each line carries a UTC timestamp, the process id and a level:

| Level   | What goes there                                                                        |
| ------- | -------------------------------------------------------------------------------------- |
| `error` | why a command failed                                                                   |
| `warn`  | failed hooks, failed uninstalls, files linkfix skipped, download retries               |
| `info`  | everything the command printed                                                         |
| `debug` | file only: every file linkfix changed, every download (URL, size, time) or cache hit   |
| `trace` | not used yet                                                                           |

`--log-level <LEVEL>` (or `LVPM_LOG`) sets the lowest level written; the default is
`debug`, and `off` writes nothing. The console output is the same at every level.
At 10 MB the log moves to `lvpm.log.1`, replacing the previous one. Outside a
venv, and for `lvpm venv remove`, the log is the user's own:
`%LOCALAPPDATA%\lvpm\logs\lvpm.log` on Windows.

How it works: the `packages/` directory is an [LVAddons](https://labviewwiki.org/wiki/LVAddons)
location, one addon per package, each mirroring the LabVIEW install dir
under `<pkg>/1/`. A LabVIEW started with `LVAddons.AdditionalLocations`
pointing there overlays them onto its own tree, so a package in the venv
resolves as `<vilib>/...` exactly as a globally installed one would. The VIs
you write against it link portably, and `C:\Program Files` never changes.
Needs LabVIEW 2024 Q1 or later.

Environments created with the old layout must be recreated: close their
LabVIEW instance, then run `lvpm venv remove`, `lvpm venv create`, and
`lvpm install`. Removing the venv also removes its installed packages and
venv-specific settings.

Two things follow from LabVIEW reading an addon's contents at launch:

- After `lvpm install`, a LabVIEW already open on the venv cannot see the new
  files. Restart it with `lvpm launch`. For the same reason the relink runs
  in a LabVIEW that lvpm starts fresh on the venv, and is refused while one
  is already open there.
- The real installation always wins over the overlay: a package installed
  both globally and in the venv loads from the global copy, whatever its
  version. Keep the target installation clean of what the venv provides.

### Uninstalling dependencies

`lvpm uninstall <name>` warns if another installed package requires the
named package. If it has installed dependencies that are not needed by other
packages, lvpm asks whether to remove them too. Shared dependencies (and
dependencies they need) are kept and reported. In a venv, packages declared
in any group of its bound project manifest, and their transitive dependencies,
are also kept. Explicitly uninstalling a declared package warns but proceeds;
uninstall never edits the manifest.

For scripts, pass `--with-deps` to remove unshared dependencies or
`--keep-deps` to keep them without prompting. Without either option, an
uninstall that would prompt fails before removing anything when standard
input is not a terminal. `uninstall --all` still removes every package.

Installation records must contain dependency information; old or invalid
records are rejected rather than ignored. Venvs created before manifest
bindings must be removed and recreated, then their packages reinstalled.
For global installations with old records, uninstall the affected packages
with the previous lvpm version before reinstalling them.

## What an install does in LabVIEW

### Linkfix: normal package preparation

**Linkfix runs by default after copying, before PostInstall hooks.** It makes
newly written LabVIEW-tree files ready for their location: repair dependency
links and separate embedded compiled code where supported, without opening
LabVIEW. Source logic and the saved LabVIEW version are preserved; package
archives and existing unrelated files are not changed. The output shows
edits and skips rather than guessing unsupported layouts.

Use `lvpm install --no-linkfix` to opt out, or `lvpm linkfix --dry-run --verbose`
to inspect an existing tree. This reduces avoidable searches and save prompts,
but is not a replacement for setup hooks or a guarantee of package
compatibility. See [Linkfix](docs/linkfix.md) for what changes, supported
boundaries, per-file reports and the exact scope of automated validation.

### Relinking

Freshly unpacked VIs still declare the paths their build machine wrote —
`<vilib>`-style tokens for the most part, plus a residue of absolutes from the
build machine.

**The LabVIEW load/save pass is opt-in, behind `--relink`, and replaces
linkfix rather than following it.** In measurements across 10 packages saved
between LabVIEW 8.6 and 2020, unrelinked VIs load and run from a venv
untouched: LabVIEW resolves what the linker tables miss by searching vi.lib,
and an LVAddons overlay *is* vi.lib. LabVIEW can also record dependencies that
the packaged file did not list. Those observations do not mean every package
is correct without preparation or that linkfix reproduces every LabVIEW save.
`tools/linkfix/audit.py` inspects packaged paths;
`tools/linkfix/deptree.py check` checks declared dependency resolution.

With `--relink`, the pass drives a small relink VI over every installed
package folder through VI Server — load, let LabVIEW resolve the links, save —
and prints each folder's report of the files it actually rewrote. The VI is
saved for LabVIEW 2020, so any LabVIEW from 2020 on can load it.

- One pass over the whole plan, so no package is relinked before a later
  package's files are on disk.
- Overlapping folders are collapsed first: a folder nested inside another
  package's folder joins that walk instead of repeating it.
- Transient VI Server refusals (error 1000: a reference invalidated by the
  relink's own saves, or a VI not yet out of its running state) retry with a
  fresh reference and backoff.
- `lvpm relink <name>` / `lvpm relink --all` reruns the pass from the
  install manifests, without reinstalling.
- The relink VI is compiled into the `lvpm` binary and unpacked to
  `%LOCALAPPDATA%\lvpm\tools\<hash>\` on first use, so there is nothing to
  install beside the executable; `LVPM_RELINK_VI=<path>` runs a VI from disk
  instead.

### Script-VI hooks

The install hooks a package declares are extracted from the archive and run
at their proper moments. Each is handed the action-info variant hook VIs
read their parameters from (`Package Name`, `LabVIEW Target Version`,
`Quiet Mode = TRUE`, `Files Installed`, ...), so they run without parking
dialogs over the install.

| Hook               | Moment                                                                                                            |
| ------------------ | ----------------------------------------------------------------------------------------------------------------- |
| `PreInstall.vi`    | before the package's files are copied (not in a venv — its LabVIEW only starts once the files are down)           |
| `PostInstall.vi`   | after linkfix or the selected LabVIEW relink, in the venv's own LabVIEW when a venv is in effect |
| `PreUninstall.vi`  | before the package's files are removed                                                                            |
| `PostUninstall.vi` | after removal (uninstall hooks are extracted at install time; the archive is long gone when uninstall needs them) |

**Which hooks run.** Hooks were written for VIPM installing into the LabVIEW
installation, and many change that installation: they edit palette files under
`menus\`, resave NI's VIs under `resource\`, or run `msiexec`. Run from a venv
they still change the installation, which a venv must leave alone. So:

| Destination                 | Default                         | Per dependency in `lvpm.toml`             |
| --------------------------- | ------------------------------- | ----------------------------------------- |
| venv                        | no hooks                        | `hooks = "run"` enables that package's    |
| LabVIEW installation        | all hooks, as VIPM              | `hooks = "skip"` disables that package's  |
| headless (`LV_RTE_HEADLESS`) | no hooks                        | `hooks = "run"` enables that package's    |

```toml
[dependencies]
my_lib_with_setup = { version = "2.1.0.4", hooks = "run" }
wiresmith_technology_lib_g_cli = { version = "3.0.1.98", hooks = "skip" }
```

`--no-hooks` runs none and `--hooks` runs all, overriding the manifest. A
PostInstall hook the policy holds back is recorded as `skipped` (see
`lvpm list`). It runs at the next install once the policy allows it, or now
with `lvpm run-hooks <name>`, which runs a named package's hook whatever the
policy says. `lvpm run-hooks --all` follows the policy. Uninstall hooks never
run in a venv: its LabVIEW has to be closed before the files can go.

Measured on the 22 hooks of a 50-package project: six are harmful from a
venv, and the rest open documentation pages, recompile examples or leave
markers. The one that was needed, NI's Advanced HTTP Client fixing the
Call Library path its package builder broke, is now done by linkfix's
`clfn-wildcard` method (see [Linkfix](docs/linkfix.md#call-library-wildcards)).

Hook failures are reported, never fatal: the files are installed either way,
and an uninstall is never left half-done. After the hooks, a global `install`
rebuilds the palettes and menus so a package's palette and Tools entries
appear without restarting LabVIEW; `lvpm refresh` does that step alone.

### VI Server

The transport underneath relink and hooks is a typed VI Server TCP client
(`viserver.rs`): no LabVIEWCLI, no ActiveX. It speaks the flattened-data
formats (scalars, strings, paths, n-dimensional arrays, and variants with
named attributes) and is documented in
[docs/vi-server-protocol.md](docs/vi-server-protocol.md). lvpm reads the
target's port from its `LabVIEW.ini` (LabVIEW 2025 defaults to 3363, 2026 to 3364) and starts the IDE when nothing answers there. Only relinking and hooks
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
on one line and each overridable (the relink pass is off on both, behind
`--relink`):

|                                    | developer's machine                                                                 | headless (`LV_RTE_HEADLESS`)                                                                                                         | override                         |
| ---------------------------------- | ----------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------ | -------------------------------- |
| `lvpm.toml` with no venv beside it | refused: a developer inside a project must not install into the machine by accident | the machine is the sandbox: install into the LabVIEW the manifest's `labview` names (a newer one may stand in, an older one may not) | `--labview-version`, `--project` |
| install hooks                      | run, before and after the files                                                     | skipped, and `lvpm list` says so: the hooks measured write a marker file and add a palette entry, IDE furniture                      | `--hooks`                        |
| palettes and menus                 | rebuilt after the install                                                           | not rebuilt: no one sees a palette                                                                                                   | `lvpm refresh`                   |

Together those mean a headless `lvpm install` never starts LabVIEW: it is
downloads, MD5 checks, file copies and linkfix preparation. Earlier DQMH timings
before default linkfix were:
two minutes with hooks, twelve with relink as well, and the mass
compile afterwards reported nothing wrong either way.

The hook skip is the one with a caveat. A hook is arbitrary LabVIEW code,
and a few packages do real setup in theirs: NI's HTTP client, for one, does
not open a session until its PostInstall has run. If a package behaves
differently in the container than on your machine, `--hooks` is the first
thing to try; it starts a headless LabVIEW for them. `--relink` likewise
when the job's output _is_ relinked packages.

NI's Linux image (`nationalinstruments/labview:2026q3-linux`) works the same
way; [docs/linux.md](docs/linux.md) has the differences, and
[.github/workflows/build.yml](.github/workflows/build.yml) runs lvpm in it on
every push.

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

| Source                                                    | Packages |
| --------------------------------------------------------- | -------- |
| `download.ni.com/evaluation/labview/lvtn/vipm/index.vipr` | ~2,530   |
| `www.jkisoft.com/packages/jkisoft.ogpd`                   | ~2,290   |

Both are OGPM's Package Directory format, `[Package <name>-<version>]`
sections with a `Package.URL`, unchanged since `openg.ogpd` in 2004; `.vipr`
adds an MD5 per entry. Anything they lack can come from an index folder of
your own, named in the manifest's `[sources]`. A `[sources]` entry is an
`http://` or `https://` folder holding an `index.vipr`. Local directories of
`.vip` files are configured globally with `lvpm config set sources.local`
instead; lvpm reads their specs and package files directly. These local
directories reuse cached spec metadata when file size and modification time
match; installation still reads their original archives unless imported.

Use `lvpm cache add` to import a `.vip` or `.ogp`, or all such files directly
inside a directory, into the shared content-addressed cache:

```powershell
lvpm cache add D:\LabVIEW\Packages\some_lib-1.2.3.vip
lvpm cache add D:\LabVIEW\Packages
lvpm --refresh cache add D:\LabVIEW\Packages
```

Imports record SHA-256, MD5, package metadata, and the original path. Repeated
imports skip unchanged files when their cached blob still exists; `--refresh`
reads and hashes them again. Imported packages are available to search and
installation even after the originals are deleted. Cache reads verify the
SHA-256 and installation also checks MD5. Local index files and cache
list/remove/prune commands are not implemented yet.

Remote package installs check the cache by name, version, and the feed's MD5
before downloading. A single distinct SHA-256 match is verified and reused,
including archives imported from disk. Missing, corrupt, or ambiguous matches
are downloaded and verified, then stored with SHA-256 and MD5 for future runs.
Packages without a feed MD5 are stored but downloaded again on later installs:
name/version alone does not identify their bytes. `--refresh` refreshes feeds,
not already verified package blobs. Dry runs may populate the cache without
writing package files into the target.

Remote feeds, downloads, and imported packages are recorded in `%LOCALAPPDATA%\lvpm\cache`, in a
SQLite index (`lvpm-cache.db`) over a content-addressed store of the bodies
themselves. For feed bodies, without `--refresh` the stored body is used and
no feed request goes out. With it, the `ETag` and `Last-Modified` the server last gave
are sent back as a conditional request: only a feed that actually changed is
downloaded again, and an unchanged one answers `304` and costs no bytes. See
[Configuration](#configuration) to move the cache.

## Configuration

Settings are named the way npm names `npm_config_<key>`: the environment
variable is `LVPM_CONFIG_<KEY>` (or lowercase `lvpm_config_<key>`), and an
empty value counts as unset. Precedence, highest first:

1. Environment variable `LVPM_CONFIG_<KEY>`.
2. User config file at `%LOCALAPPDATA%\lvpm\config.toml` (Linux:
  `$XDG_CONFIG_HOME/lvpm/config.toml`, else `~/.config/lvpm/config.toml`),
  written by `lvpm config set`.
3. Built-in default.

| Variable | Default | What it sets |
| --- | --- | --- |
| `LVPM_CONFIG_CACHE` | `%LOCALAPPDATA%\lvpm\cache` (Linux: `$XDG_CACHE_HOME/lvpm/cache`, else `~/.cache/lvpm/cache`) | Where the cache DB, feed bodies, and imported package archives live |
| `LVPM_CONFIG_SOURCES_LOCAL` | Empty | Local `.vip` directories; separate paths with `;` on Windows or `:` on Linux |
| `LVPM_CONFIG_VENV_DIR` | `.lv-venv` | Project-relative venv directory (or an absolute path) |

```powershell
lvpm config set sources.local D:\LabVIEW\Packages
lvpm config get sources.local
lvpm config set sources.local                 # clear configured local sources
lvpm config set venv.dir .venv
lvpm config get venv.dir
# Or set LVPM_CONFIG_SOURCES_LOCAL for this shell; it overrides config.toml.
$env:LVPM_CONFIG_SOURCES_LOCAL = 'D:\LabVIEW\Packages;E:\MorePackages'
# Relative venv.dir values resolve from each project root; absolute paths are used as-is.
$env:LVPM_CONFIG_VENV_DIR = '.venv'
lvpm search caraya
```

In CI, set it on the job to keep the cache on a persistent volume between
runs.

## Known limitations

- `Replace Mode = If Newer` only writes when the file is absent; doing it
  properly means comparing archive timestamps against disk.
- No dependency conflict resolution: each package gets the newest version
  meeting the highest floor anything asks for, and a manifest pin always
  wins (with a warning when a dependent asks for more).
- No mass compile; LabVIEW recompiles installed VIs on first load.
- A hook VI's own `error out` is not read back. A hook that fails internally
  but runs to completion reports as ok.
- A failure part-way through unpacking leaves files behind with no manifest,
  and one failed download aborts the rest of the plan.
- Windows is the tested platform. Linux is verified against LabVIEW 2026 Q3
  in NI's container image only, and does not yet honour a package's
  `Exclusive_OS` ([docs/linux.md](docs/linux.md)).
- In a venv, `PostInstall` hooks run in the venv's own LabVIEW but
  `PreInstall` ones do not (that instance only starts once the files are
  down), and file groups aimed at machine locations (`<OS ...>`, `<temp>`) are
  skipped — both are recorded in the manifest and shown by `lvpm list`. A hook
  that drops palette files into the overlay is picked up at the next
  `lvpm launch`, not by a refresh. Uninstall hooks still never run in a venv,
  whose LabVIEW has to be closed before its files can be removed.
  `lvpm install <name>` in a venv does not
  add the pin to `lvpm.toml`. No warning yet when a global copy of a venv
  package shadows it.

[docs/roadmap.md](docs/roadmap.md) has what is planned.

## Layout

| Module                         | Responsibility                                                                                                          |
| ------------------------------ | ----------------------------------------------------------------------------------------------------------------------- |
| `main.rs`                      | The CLI: commands, target and venv selection, and install/uninstall orchestration                                       |
| `logging.rs`                   | The per-venv log file, and the `log!` family of macros that print a line and log it                                    |
| `index.rs`                     | Fetch and parse package directories (`.ogpd` / `index.vipr`), resolve names to versions                                |
| `resolve.rs`                   | Resolve requested packages and dependencies into an installation order                                                  |
| `dependency.rs`                | Package dependency metadata and dependency-aware uninstall planning                                                     |
| `fetch.rs`                     | Acquire package bytes: cache lookup, downloads, integrity verification, and cache population                            |
| `cache.rs`                     | The cache: the SQLite index of known sources and the content-addressed store of what they served                        |
| `spec.rs`                      | The `spec` manifest inside a package (the OGPT `.ogp` format and VIPM's `.vip` dialect)                                 |
| `project.rs`                   | The project manifest `lvpm.toml`: dependencies and their constraints, sources, minimum LabVIEW, lookup from a directory |
| `target.rs`                    | LabVIEW detection, and where each `Target Dir` token points                                                             |
| `install.rs`                   | Plan, unpack, extract hook VIs, record a manifest, uninstall                                                            |
| `venv.rs`                      | A project's `.lv-venv/` venv by default: binding, lookup from the working directory, `lvpm venv`                       |
| `launch.rs`                    | Start LabVIEW on a venv: the `-pref` ini, readiness by handshake, detached spawning                                     |
| `relink.rs`                    | Drive the embedded relink VI over installed folders; folder collapsing and retries                                      |
| `refresh.rs`                   | Rebuild palettes and menus through LabVIEW's own shipping VIs                                                           |
| `viserver.rs`                  | The VI Server TCP protocol: connection, methods, flattened data                                                         |
| `version.rs`                   | Package version ordering: OGPT `version-release`, VIPM's four-part form; not semver                                     |
| `src/lv-src/relink-package.vi` | The relink VI embedded into the executable at build time (LabVIEW 2020)                                                 |
| `scripts/install.ps1`          | Download, verify and install the latest release                                                                         |
| `e2e/`                         | `lvpm-e2e`: end-to-end checks against LabVIEW for Linux and for Windows in NI's container images, with local test packages and with public ones (the LabVIEW tree must come out of install and uninstall unchanged) |

## License

MIT OR Apache-2.0
