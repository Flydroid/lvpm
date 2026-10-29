# Using lvpm

## Commands

| Command                                              | What it does                                                                                                                                 |
| ---------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| `lvpm install <name>[@<version>]`                    | Outside a project, or with `--global`, `--prefix` or `--labview-version`: resolve the package and its dependencies, download, verify MD5, unpack, relink, run hooks. In a project it is `lvpm add` |
| `lvpm install`                                       | Inside a project: the same as `lvpm sync`                                                                                                    |
| `lvpm sync`                                          | Make the destination hold exactly what `lvpm.lock` lists, one plan with one relink pass ([The lock file](#the-lock-file))                    |
| `lvpm add <name>[@<constraint>]` / `--dev`           | Add a dependency to `lvpm.toml`, lock and sync                                                                                               |
| `lvpm remove <name>` / `--dev`                       | Remove a dependency from `lvpm.toml`, lock and sync                                                                                          |
| `lvpm lock` / `--check` / `--upgrade`                | Write `lvpm.lock`, check it, or move packages to newer versions                                                                              |
| `lvpm uninstall <name>` / `--all`                    | Remove exactly the files that were installed and prune empty folders                                                                         |
| `lvpm list`                                          | What lvpm has installed into the selected target, and which hooks it did not run                                                             |
| `lvpm search <query>` / `--all [query]`              | Search the package indexes; `--all` lists every version of every package and its source                                                      |
| `lvpm info <package>[@<version>]`                    | Show a package's description, requirements, files and versions, from its archive (downloaded, then cached)                                   |
| `lvpm targets`                                       | Detected LabVIEW installations                                                                                                               |
| `lvpm relink <name>` / `--all`                       | Rerun the relink pass from the install manifests, without reinstalling                                                                       |
| `lvpm run-hooks <name>` / `--all`                    | Rerun a package's PostInstall hook, for one that failed during install                                                                       |
| `lvpm refresh`                                       | Rebuild palettes and the File/Tools/Help menus from disk (`install` does this itself)                                                        |
| `lvpm start`                                         | Start the selected LabVIEW and wait until its VI Server answers                                                                              |
| `lvpm venv create` / `status` / `remove`             | Manage the project's venv (see [Per-project dependencies](#per-project-dependencies-venvs))                                                  |
| `lvpm launch [<lvproj>]`                             | Start a LabVIEW on the project's venv                                                                                                        |
| `lvpm build`                                         | Package the project's library as `dist/<name>-<version>.vip`, from `[distribution]` ([Building a package](#building-a-package))              |
| `lvpm publish --to <folder>`                         | Copy the built package into a folder feed; a published file is never replaced ([Publishing a package](#publishing-a-package))                |
| `lvpm vi-probe` / `lvpm vi-run` / `lvpm vi-save` / `lvpm app-probe` | VI Server primitives, for diagnosis (see [VI Server](labview-integration.md#vi-server))                                                                            |

Flags that apply to every command:

| Flag                       | Meaning                                                                                                   |
| -------------------------- | --------------------------------------------------------------------------------------------------------- |
| `--labview-version <YYYY>` | Install into this LabVIEW version (see `lvpm targets`)                                                    |
| `--project <DIR>`          | Use that project's venv instead of looking upwards from the current directory                             |
| `--global`                 | Use the LabVIEW installation itself, even from inside a project that has a venv                           |
| `--prefix <DIR>`           | Install into a scratch directory instead of LabVIEW (file copy only; nothing to relink or run hooks with) |
| `--refresh`                | Ask each index whether it changed (a conditional request) instead of trusting the cache in `%LOCALAPPDATA%\lvpm\cache` |
| `--json`                   | Write JSON Lines to stdout for a script or front end ([JSON output and exit codes](../README.md#json-output-and-exit-codes)). Not taken by `app-probe`, `vi-probe`, `vi-save` or `vi-run` (exit 2) |

`install` flags: `--dry-run` shows every file, relink folder and hook and
changes nothing; `--no-deps` skips dependencies and `--allow-downgrade`
permits replacing an installed package with an older version (both only for
an install that is not a sync or an `add`: a lock always holds the full set);
`--no-relink` skips the
relink pass and `--relink` forces it where it is off by default; `--hooks`
runs install hooks where they are off by default (both: a headless LabVIEW,
see [CI in a container](ci-docker.md)). `--relink-timeout <SECS>`
(default 900) bounds one folder's relink; `--keep-open` leaves the LabVIEW
started for a venv relink running. `sync`, `add` and `remove` take `--dry-run`
and the relink and hook flags as well.

## The project manifest

`lvpm.toml` at the repo root says what a project depends on and which hosted
indexes to use. Local package folders are configured globally instead. Any
`lvpm` command run at or below it picks it up; hosted `[sources]` apply to
`search` and to a single-package `install` as much as to a full one.

```toml
[project]
name = "NovaXC"
version = "0.1.0"
labview = ">=2025"                # the oldest LabVIEW the project is meant for

[sources]                         # repositories beyond the public indexes
mirror = "http://host:8090/files" # a hosted index.vipr folder
defaults = true                   # false: only the sources listed here

[dependencies]
delacor_lib_qmh = "7.1.2.1547"    # exactly this version
oglib_error = ">=6.0.1"           # the newest version satisfying the floor
jki_lib_state_machine = "*"       # the newest version

[dev-dependencies]                # test and CI tooling; same syntax; skipped with --no-dev
jki_lib_caraya = "*"

[nipm.dependencies]               # NI Package Manager packages: reported, not installed
ni-daqmx-labview-support = "26.0"
```

`labview` is a minimum, not a pin: a venv binds to that version or a newer
one. Dependencies a package carries are always floors, so what a project pins
exactly stays exact and the rest resolves to the newest version that fits.

Before 0.6, `labview` was a bare year. lvpm now refuses `labview = "2025"`
and asks for `labview = ">=2025"`; change the line and nothing else.

## The lock file

`lvpm.toml` is what you ask for, with loose constraints. `lvpm.lock`, beside
it, is what you got: every package, direct or transitive, with its exact
version, SHA-256 and source. lvpm writes it; do not edit it. Commit both.

```console
$ lvpm lock                 # resolve lvpm.toml, write lvpm.lock
$ lvpm sync                 # make the project's venv hold exactly what lvpm.lock lists
```

A teammate or a CI job that syncs the same lock gets the same packages,
byte for byte. A new release of a package never changes the lock; only an
upgrade does.

**Locking.** `lvpm lock` picks, for every package, the newest version that
satisfies all the constraints and runs on the manifest's `labview` (locking
refuses a manifest without one). If the constraints cannot all hold, it names
the ones in conflict and writes nothing. It reads `lvpm.toml` and the package
sources, never what is installed, and leaves a lock that still represents the
manifest as it is. A requirement that no source lists is recorded as
unresolved and reported as a warning; a direct dependency that cannot be found
fails. Constraints are exact (`7.1.2.1547`), `>=` or `*`.

**Out of date.** A lock is out of date when a dependency was added or removed,
a constraint no longer admits the locked version, `labview` changed, or the
hosted `[sources]` changed. New releases upstream do not make it so, and
neither does the per-user `sources.local`. `lvpm lock --check` writes nothing:
it exits 0 if `lvpm.lock` exists and is current, and otherwise exits non-zero
with the reason on one line. It reads no source, so it is cheap.

**Syncing.** `lvpm sync` installs what is missing, replaces what is at another
version, and uninstalls the lvpm-installed packages the lock does not list. It
prints each change and why before it acts, and does not ask. `--dry-run`
prints that plan and changes nothing, not even the lock. Each archive is taken
from the cache by SHA-256, else from any source with the same name and version
whose SHA-256 matches; one that does not match is never installed. If none
does, sync fails naming the package, version and SHA-256, and tells you to
`lvpm cache add <file>` the archive or point `sources.local` at a folder that
has it. The destination is the project's venv, or the LabVIEW installation
with `--global`. Packages from `[dev-dependencies]` are synced too; `--no-dev`
leaves them out and uninstalls them if they are there.

With no lock, or an out-of-date one, `lvpm sync` makes a new one first. To
refuse that instead:

| Flag          | When the lock is missing or out of date                                       |
| ------------- | ----------------------------------------------------------------------------- |
| (none)        | Lock again, then sync                                                         |
| `--locked`    | Fail and change nothing                                                       |
| `--frozen`    | Sync the lock as it is, even out of date; fail only if there is no lock       |
| `--no-locked` | Lock again, even where `--locked` is the default (see [CI](ci-docker.md)) |

### Adding and removing dependencies

```console
$ lvpm add oglib_error              # writes oglib_error = ">=<the version it locked>"
$ lvpm add "gamma@>=0.5"            # with a constraint
$ lvpm add --dev jki_lib_caraya     # into [dev-dependencies]
$ lvpm remove gamma                 # --dev if it is in [dev-dependencies]
```

`add` and `remove` edit `lvpm.toml` (comments and layout stay), then lock and
sync. If the lock or the sync fails, `lvpm.toml` and `lvpm.lock` are put back
as they were. A package that another one still requires stays in the lock
after `remove`, and the output says so.

**Quote a constraint.** PowerShell and cmd read `>` as a redirect, so
`lvpm add gamma@>=0.5` sends lvpm's output into a file named `=0.5` and hands
lvpm only `gamma@`. Put the argument in double quotes, as in
`lvpm add "gamma@>=0.5"`; that works in both.

In a project, `lvpm install <name>` with no destination flag is `lvpm add
<name>`, and says so in one line; a bare `lvpm install` is `lvpm sync`. With
`--global`, `--prefix` or `--labview-version`, or outside a project,
`lvpm install <name>` is the plain install described above and changes no
manifest or lock.

### Upgrading

Locked versions stay put until you ask:

```console
$ lvpm lock --upgrade                         # everything to the newest version the constraints allow
$ lvpm lock --upgrade-package gamma           # only gamma, to its newest
$ lvpm lock --upgrade-package gamma@1.2.0     # only gamma, to 1.2.0; repeat the flag for more packages
$ lvpm sync
```

With `--upgrade-package`, the other packages stay where the lock has them
unless they cannot. If the manifest's constraint excludes the version asked
for, lvpm rewrites it and keeps its kind: an exact pin becomes the new
version, `>=1.0` becomes `>=` the new version. A `*` is never rewritten.
There is no separate `update` command.

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

$ lvpm install                    # sync: exactly what lvpm.lock lists, into .lv-venv
$ lvpm add oglib_error            # one more dependency: lvpm.toml, lvpm.lock and .lv-venv
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
  on a headless machine (see [CI in a container](ci-docker.md)).
  Every venv-mode command prints which venv it resolved to.
- `lvpm launch [<lvproj>]` starts a LabVIEW on the venv: a copy of the
  target's `LabVIEW.ini` with the venv mounted and VI Server on a port of its
  own, handed over with `-pref`. Your primary LabVIEW, if open, is untouched.

How it works: the venv directory is an [LVAddons](https://labviewwiki.org/wiki/LVAddons)
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

### The resave

A venv install opens and saves each package's VIs in the venv's LabVIEW
before the package reaches `.lv-venv`. A VI saved in an older LabVIEW can
otherwise load broken from an add-on folder. The package is unpacked into
`.lv-venv.staging/` beside the venv, resaved there, then moved into place.

```console
starting the venv's LabVIEW for the resave (about a minute per package)
resave (1/2) oglib_error ... 12 saved (48s)
resave (2/2) oglib_string ... already saved in LabVIEW 2026, skipped
```

- It costs one LabVIEW start per run and about 40 to 85 seconds per package,
  mostly the cold load of its VIs. A package whose VIs are all saved in the
  venv's LabVIEW version already is skipped, and starts no LabVIEW.
- `--no-relink` and headless installs skip only the relink. The resave
  still runs, and the relink line says so. `--dry-run` lists the packages with
  VIs it may resave, including ones the version check will skip, and starts
  no LabVIEW.
- A failed resave is a warning, not an error: the package is still
  installed and the exit code stays 0. The warning names the package and its
  folder. Reinstall the package; if it fails again, run `lvpm launch` and
  mass compile that folder. A recap at the end lists every package that
  failed. If the venv's LabVIEW cannot start at all, one warning lists the
  packages installed without a resave.
- Scripts read the failures from `--json`: the install summary carries a
  `resave_failed` list, present only when a package failed.
- A venv installed before this release has packages that were never
  resaved. Delete `.lv-venv` and run `lvpm sync` to reinstall them.

## Building a package

`lvpm build` packages one LabVIEW library as a `.vip` that `lvpm install`
installs. It needs no LabVIEW. VIPM refuses this `.vip`, so it is for lvpm
only; a later release writes lvpm's own `.lvpkg` instead and drops `formats`
and `[distribution.vip]` ([ADR 6](adr/0006-lvpkg-only-vip-read-only.md)). Describe the package in
`lvpm.toml`:

```toml
[project]
name    = "hse_lib_hse_logger"   # the package name, unless [distribution.vip] name is set
version = "2.2.2.114"            # 3 or 4 numeric parts; 2.2.2 is written as 2.2.2.0
labview = ">=2016"               # becomes the package's LabVIEW gate

[distribution]
top-level-library = "Source/HSE-Logger/hse-logger.lvlib"  # required; relative to lvpm.toml
kinds   = ["source"]             # optional; only ["source"] for now
formats = ["vip"]                # optional; only ["vip"] for now

[distribution.vip]
name         = "hse_lib_hse_logger"          # optional; required when [project] name is scoped
vendor       = "Hampel Software Engineering" # required
display-name = "HSE Logger"                  # required
install-dir  = "vi.lib/Hampel Software Engineering/HSE Logger"  # optional; this is the default

[distribution.vip.dependency-names]          # required for each scoped dependency
"@hexagon/lib-a" = "hexagon_lib_a"
```

- The package holds the library's members as the `.lvlib` lists them: its
  VIs and controls, its classes and their members, and every file in an
  auto-populating folder. A file in the library's folder that the library
  does not list is left out. Every member must be on disk and inside the
  library's folder.
- Members install below `install-dir` with the layout they have below the
  library's folder.
- `[dependencies]` become the package's `Requires`: `"1.0"` and `">=1.0"`
  both as `name>=1.0`, `"*"` as the bare name.
- The package is written to `dist/<name>-<version>.vip`. The same sources
  give the same bytes. A failed build names the key or file at fault and
  leaves an earlier package in `dist/` as it was.
- Any other key under `[distribution]` or `[distribution.vip]` is an error
  that names the key.
- Paths in `[distribution]` use forward slashes and stay inside their
  folder: `top-level-library` inside the project, `install-dir` below the
  LabVIEW folder. A listed member that is a link to a file outside the
  library's folder is refused; links inside an auto-populating folder are
  skipped.
- The package name takes letters, digits, `_`, `-` and `.`. A quote or line
  break in `vendor`, `display-name` or a dependency name is refused, since
  it would end up in the package's spec.

## Publishing a package

`lvpm publish --to <folder>` copies the package `lvpm build` wrote for the
current manifest, `dist/<name>-<version>.vip`, into a folder feed. Another
project that lists that folder in `sources.local` then locks and installs
it.

```console
$ lvpm build
$ lvpm publish --to D:/feeds/packages
published D:\feeds\packages\demo_lib-1.2.3.0.vip
```

- `--to` is required. The folder must exist; a relative path is read from
  where the command runs, not from the project.
- A published file is never replaced. If the folder already holds a file of
  that name, publish fails, names it, and leaves it as it was: give the
  project a new version and build again.
- Only the current version is published. Older packages in `dist/` stay
  where they are.
- Publish does not build. With no package in `dist/` for the current
  manifest it fails and says to run `lvpm build`.
- The copy is written beside its final name and linked into place, so a
  reader of the folder never sees half a package. That needs a file system
  with hard links (NTFS and ext4 have them); on one without them, publish
  fails and names the folder.

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
