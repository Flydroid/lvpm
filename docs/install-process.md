# What an install does in LabVIEW

After the files are copied, an install prepares them for their new location
(linkfix, or optionally a LabVIEW relink), runs the package's install hooks,
and rebuilds palettes and menus. Only the relink and hooks need a running
LabVIEW; lvpm talks to it over VI Server.

## Linkfix: normal package preparation

**Linkfix runs by default after copying, before PostInstall hooks.** It makes
newly written LabVIEW-tree files ready for their location: repair dependency
links and separate embedded compiled code where supported, without opening
LabVIEW. Source logic and the saved LabVIEW version are preserved; package
archives and existing unrelated files are not changed. The output shows
edits and skips rather than guessing unsupported layouts.

Use `lvpm install --no-linkfix` to opt out, or `lvpm linkfix --dry-run --verbose`
to inspect an existing tree. This reduces avoidable searches and save prompts,
but is not a replacement for setup hooks or a guarantee of package
compatibility. See [linkfix.md](linkfix.md) for what changes, supported
boundaries, per-file reports and the exact scope of automated validation.

## Relinking

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

A package whose VIs only LabVIEW can repair can be marked `relink = true` in
the manifest; it is then relinked after linkfix (see
[manifest.md](manifest.md#dependency-tables-hooks-and-relink)).

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

## Script-VI hooks

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
markers. The one that was needed is NI's Advanced HTTP Client's. It repairs
the Call Library path that VI Package Builder broke when it built the package,
a known VIPM issue: [VI Package Builder removes cross platform wildcards from
Call Library Function nodes during the build process](https://support.vipm.io/hc/en-us/articles/360053941572-VI-Package-Builder-removes-cross-platform-wildcards-from-Call-Library-Function-nodes-during-the-build-process).
Linkfix's `clfn-wildcard` method now makes that repair itself (see
[linkfix.md](linkfix.md#call-library-wildcards)). Dataflow G's G-Image
works around the same issue differently: its hook unzips clean copies of its
VIs over the installed ones. linkfix covers that one too.

Hook failures are reported, never fatal: the files are installed either way,
and an uninstall is never left half-done. After the hooks, a global `install`
rebuilds the palettes and menus so a package's palette and Tools entries
appear without restarting LabVIEW; `lvpm refresh` does that step alone.

## VI Server

The transport underneath relink and hooks is a typed VI Server TCP client
(`viserver.rs`): no LabVIEWCLI, no ActiveX. It speaks the flattened-data
formats (scalars, strings, paths, n-dimensional arrays, and variants with
named attributes) and is documented in
[vi-server-protocol.md](vi-server-protocol.md). lvpm reads the
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
