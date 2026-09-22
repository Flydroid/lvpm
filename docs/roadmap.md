# Roadmap

What lvpm does next, and why. Items are ordered within each section by how
much they unblock. Done items stay for a while so the reasoning is on record.

The origin is the OpenG Package Tools design
([docs/ogpm-model.md](ogpm-model.md)): most of what is below — the lockfile
and venv (OGPM's "Development System Configuration Manager"), `lvpm check`
(its global VI namespace database), `lvpm tree` (Package Query), conflicts,
verification, the package cache, a builder that writes the spec — was
specified there in 2002–2005 and never finished. The OGPT → lvpm table in that
document is the checklist.

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

- **One-command install of a released `lvpm.exe`.** A GitHub Release per tag
  carrying `lvpm.exe` (built by CI, so the relink VI embedded in it is the one
  from that commit), and an `install.ps1` at a stable URL for
  `irm https://…/install.ps1 | iex`: downloads the latest release (or
  `-Version x.y.z`), verifies its checksum, drops it in `%LOCALAPPDATA%\lvpm\bin`,
  adds that to the user `PATH`, prints `lvpm --version`. Motivation: a stale
  `cargo install` in `~\.cargo\bin` was still looking for
  `tools\Relink Package.vi` on disk long after it had been embedded — the
  binary people run has to be the one that was released, not whatever tree
  they built last. Same script serves a container image (`RUN irm … | iex`).
- **An lvpm package format** built without LabVIEW: the source layout is
  already LVAddons-native; check VI versions before packaging; post-build
  steps (error handling, etc.).
- **Registries with existing tooling** (conda/rattler-style) or git-based
  sources; **git repo as a package source** for install.
- **PPL support.**
- **A package cache** shared between venvs, so a second project installing
  the same version copies instead of downloading.
- **Fetch over HTTPS.** `jkisoft.com` 301-redirects both the directory and
  every package to `s3-us-west-1.amazonaws.com/jki-vi-package-network` over
  plain HTTP, and the `Package.MD5` we check against comes over that same
  channel — so today the hash catches corruption, not tampering. Try `https`
  on both hops (S3 and download.ni.com both serve it) and fall back only if a
  source has no TLS.
  - Own `User-Agent` stays `lvpm/<version>`: honest, and the first thing to
  check if a public index starts closing connections on us.

## From the OGPM design, not yet started

- **`lvpm verify <pkg>`** — OGPM Package Verification: compare installed files
  against the install manifest (size, hash), run the package's `Verify` script
  VI if it has one, report anomalies.
- **`Conflicts`** — parse it (same grammar as `Requires`), refuse to install
  over a conflicting package unless told to uninstall it first.
- **Upgrade** as OGPM defines it: uninstall the old version, then install the
  new one, in one command.
- **`Requires` operators beyond `>=`** — `<`, `<=`, `=`, `>`, and the optional
  release; today only the floor is honoured.
- **Per-file-group platform gates** — `Exclusive_OS` /
  `Exclusive_LabVIEW_Version` on a `[File Group N]`, not just on the package.

## Housekeeping

- `Replace Mode = If Newer` compares timestamps instead of "write when absent".
- Read hook VIs' `error out` back.
- A failure part-way through unpacking leaves files behind with no manifest.
- Linux target detection exists but is untested.
