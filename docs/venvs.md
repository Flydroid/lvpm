# Per-project dependencies (venvs)

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
  on a headless machine (see [ci-container.md](ci-container.md)).
  Every venv-mode command prints which venv it resolved to.

The manifest's format is in [manifest.md](manifest.md).

## Launching LabVIEW

`lvpm launch [<lvproj>]` starts a LabVIEW on the venv: an ini of the
venv's own (`.lv-venv/labview.ini`, copied from the target's `LabVIEW.ini` by
`lvpm venv create` without its recent-files lists, so the venv IDE never
offers projects from outside the venv; options you set in the venv IDE
persist; delete it to inherit your global settings again at the next launch)
with the venv mounted, VI Server on a port of its own, and its own
compiled-object cache, `LabVIEW Data` folder and temp directory under
`.lv-venv/`, handed over with `-pref`. Your primary LabVIEW, if open, is
untouched, and its autosave files, cache and log (`LabVIEW_*_cur.txt`) are
never shared with the venv's — look for the venv's log in `.lv-venv/temp`.
The venv starts with an empty `LabVIEW Data`, so personal customisations kept
there (icon templates, right-click plugins) are not available in it.

Before starting, `launch` checks the venv's install records against the
manifest's `[dependencies]` (`main` only): every package installed at a
version the manifest allows, all of its recorded files present, the relink
pass done where `relink = true`, and PostInstall run where the hook policy
runs it. It refuses on a mismatch and names each one; `--no-check` starts
anyway. Files edited or resaved since install are not detected.

## Layout

```text
.lv-venv/
  packages/<pkg>/1/    package addon and its mirrored LabVIEW tree
  installed/           package installation records
  venv.json            LabVIEW binding
  labview.ini          venv-specific LabVIEW settings
  logs/lvpm.log        lvpm's log of every command run on this venv
  objcache/ data/ temp/  isolated LabVIEW runtime files
```

The log is described in [configuration.md](configuration.md#logs).

## How it works

The `packages/` directory is an [LVAddons](https://labviewwiki.org/wiki/LVAddons)
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
