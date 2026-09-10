# Roadmap

What lvpm does next, and why. Items are ordered within each section by how
much they unblock. Done items stay for a while so the reasoning is on record.

## Per-project dependencies (venvs)

A project keeps its packages under `.project/`, mounted into LabVIEW as an
LVAddons location. Background and the measurements behind the design: the
overlay gives `<vilib>`-relative linkage (portable, identical to a global
install); LabVIEW reads addon contents at launch; the real install always
wins over the overlay; `LVAddons.AdditionalLocations` needs LabVIEW 2024 Q1.

### Iteration 1 — done

- `lvpm venv create | status | remove`; binding to a LabVIEW version (the
  manifest's `labview` is the minimum).
- Destination resolution without an activation step: `--global` / `--prefix`
  explicitly, else the nearest venv above the working directory.
- `install` into a venv: one addon per package, `lvaddoninfo.json`, machine
  locations skipped and recorded, hooks skipped and recorded, relink inside a
  LabVIEW started fresh on the venv and closed again (`--keep-open`).
- `lvpm launch [lvproj]`: the target's ini with the venv mounted and VI Server
  on the venv's port, handed over with `-pref`.
- `vi-probe` / `vi-save` / `vi-run` talk to the venv's LabVIEW when one is in
  effect.
- Verified: create/status/lookup/`--global`; install + relink; clean
  uninstall; refusal to relink against a LabVIEW that predates the files;
  launch returns promptly; palettes build from the overlay (DQMH's showed
  without its PostInstall hook).

### Iteration 2

- **Install hooks in a venv.** Today none run. `ni_lib_advanced_http_client_api`
  genuinely needs its PostInstall (it repairs Call Library paths); DQMH did
  not (its palette came from the overlay). Candidate: run PostInstall inside
  the venv instance, snapshot the real installation before and after, move
  palette files it dropped into `<menus>` into the addon, warn about anything
  else. Per-dependency `hooks = "run" | "skip"` in the manifest makes it an
  explicit opt-in.
- **Relink what a partial run left behind.** A rerun after a failed install
  only relinks the packages *it* copied; packages the first run copied stay
  `NOT relinked` until `lvpm relink --all`. The venv install should relink
  everything still unrelinked in the store.
- **Survive one bad download.** A single failed download aborts the plan;
  retry transient errors, continue past a package, fail at the end with the
  list — as `uninstall --all` already does.
- **Shadowing warning.** A package installed both globally and in the venv is
  loaded from the global copy whatever its version. Warn at install and at
  launch, with both versions and the uninstall command that fixes it.
- **Duplicate logical paths across addons.** Two venv packages shipping the
  same `vi.lib` file: LabVIEW's (undocumented) scan order picks one. Warn,
  naming the file and both packages.
- `lvpm venv rebind --labview-version` (wipe + rebind in one step).
- Machine-bound file groups: `--os-writes global` to opt in per install.
- Palette / example / template discovery in the IDE for overlay content
  (`examples`, `project`, `help`, `templates`, `Targets`): a GUI check.
- Ask NI: is there a way to make a running LabVIEW re-read an addon location?

## The manifest and the lockfile

`lvpm.toml` (done: exact pins, `>=` floors, `*`, `[sources]` with relative
folders and `defaults = false`, `labview` minimum, `[nipm.dependencies]`).

- **`lvpm.lock`** — the resolved closure (name, version, source URL, MD5) so
  `lvpm install` restores the identical venv on another machine and only
  `lvpm update` re-resolves. The thing a venv without a lock only pretends to
  be.
- **`lvpm add <pkg>[@constraint]` / `lvpm remove <pkg>`** — edit the manifest
  and install/uninstall in one step (the `toml` crate can write; the lockfile
  wants this too).
- **`[dev-dependencies]`** — test and CI tooling (Caraya, VI Analyzer tests,
  DQMH validation) installed by default, skipped with `--no-dev`.
- Per-dependency tables: `{ version = "...", hooks = "run", source = "local" }`.
- `[venv]` overrides: `dir`, `vi-server-port`, `bitness`.
- Constraints beyond `>=`: `~` / `^` if they turn out to be wanted.

## Dependency analysis: `lvpm check` / `lvpm tree`

Port `tools/deptree.py` to Rust (`linker.rs`: RSRC block walker, `PTH0`
records) and make it first-class:

- `lvpm check [DIR]` — every declared dependency must resolve against the
  real install first, then each venv addon (LabVIEW's precedence). Exit
  non-zero otherwise. The post-relink correctness gate.
- `lvpm tree [DIR|VI]` — the dependency graph as text, `--cycles`, `--dot` /
  `--mermaid`.
- Known gaps in `deptree.py` to close in the port: it misses the `PTH0`
  flavour old OpenG packages use (reports "no linker tables" on every VI in
  `oglib_*`); `resolvable()` accepts a path if *any* ancestor directory exists,
  so shadowing cases pass; `.llb` archive members are skipped entirely.

## Packaging and distribution (from idea.txt)

- **An lvpm package format** built without LabVIEW: the source layout is
  already LVAddons-native; check VI versions before packaging; post-build
  steps (error handling, etc.).
- **Registries with existing tooling** (conda/rattler-style) or git-based
  sources; **git repo as a package source** for install.
- **PPL support.**
- **A package cache** shared between venvs, so a second project installing
  the same version copies instead of downloading.
- Own `User-Agent` stays `lvpm/<version>`: honest, and the first thing to
  check if a public index starts closing connections on us.

## Housekeeping

- `Replace Mode = If Newer` compares timestamps instead of "write when absent".
- Read hook VIs' `error out` back.
- A failure part-way through unpacking leaves files behind with no manifest.
- Linux target detection exists but is untested.
