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

## CI for lvpm itself: the container as the end-to-end test

lvpm's unit tests cover parsing and planning; what they cannot cover is the
part that matters — a real LabVIEW loading, relinking and compiling what
lvpm installed. NI's container image is a clean LabVIEW that is thrown away
afterwards, which is exactly the fixture that has been missing. The steps
below were run by hand on 2026-09-22 and all passed; turning them into a
workflow is the work.

- **The fixture.** `nationalinstruments/labview:latest-windows` (LabVIEW
  2026, 20 GB, ltsc2022 base) on a Windows runner with Docker in
  Windows-container mode, hyperv isolation, `-e LV_RTE_HEADLESS=1`. The
  repo mounted at `C:\g-forge`, a scratch folder mounted for logs, the
  test project mounted read-only and copied inside the container before
  anything writes to it.
- **Step 1 — the binary runs there.** `lvpm targets` lists the image's
  LabVIEW from the registry; `lvpm search` reaches both public indexes.
  Proves the MSVC build needs nothing the image lacks.
- **Step 2 — a global install with relink.** `lvpm install jki_lib_caraya
  --labview-version 2026 --relink`: 10 packages, 1386 files, 14 folders
  relinked, `lvpm list` shows every package `relinked`. This is the test of
  the VI Server client against a headless LabVIEW: the handshake retry
  (error 63 for the first ~8 s after the port opens), `Relink Package.vi`
  materialising from the binary, load/save over the wire — and, with an
  explicit `lvpm refresh --labview-version 2026` afterwards, the palette
  and menu rebuild that headless mode otherwise skips. About ten minutes; the slow
  step is Caraya's 358 s folder, so a smaller package set would do for a
  gate and this one for a nightly.
- **Step 3 — the headless default path.** A project directory holding an
  `lvpm.toml` (DQMH pinned at 7.1.2.1547 — `C:\Temp\GDevCon` was the
  specimen) and no venv; `cd` into it and run `lvpm install` with no flags.
  Expected: the headless banner naming the manifest's LabVIEW, 6 packages /
  1668 files, relink, hooks and palette refresh all reported as skipped,
  `lvpm list` from the same directory resolving the same way and listing
  the skipped hooks, and — the point — no `LabVIEW.exe` process at any time
  (`tasklist`). Seconds. With `--hooks` the same install runs 6 PreInstall
  and 6 PostInstall hooks `ok` in about a minute.
- **Step 4 — the product compiles against it.** `LabVIEWCLI -OperationName
  MassCompile -DirectoryToCompile <project copy> -MassCompileLogFile <log>
  -Headless`; assert `MassCompile operation succeeded` and a log with no
  bad-VI lines. This is the claim behind "relink off in CI": the compile
  resolves the links of an unrelinked install itself. 47 s on the DQMH
  project.
- **Step 5 — the tests run.** `LabVIEW.exe "<vi.lib>\addons\_JKI
  Toolkits\Caraya\Caraya CLI.vi" -- -s <tests folder> -x <junit.xml>` —
  Caraya's own entry point, which needs no LabVIEW to be running beforehand
  (an existing instance swallows the open and the arguments with it — seen
  once, when hooks had left one behind). Assert the JUnit file exists and
  has no `<failure>`. Caraya's shipped `tests\asserts` folder serves until
  the fixture project has tests of its own.
- **Step 6 — the container exits.** Caraya CLI quits LabVIEW itself
  (`-q` defaults on); a job that used `--hooks` or `--relink` ends with
  `LabVIEWCLI -OperationName CloseLabVIEW -Headless` instead. Assert the
  container is gone.
- **What to archive.** lvpm's stdout, the mass-compile log, and
  `%TEMP%\LabVIEW_64_*_headless_*_cur.txt` from inside the container — the
  latter is where a headless LabVIEW puts what would have been a dialog.
- **Open questions before it is a gate.** Where the 20 GB image lives
  (pull time on a fresh runner is the whole budget; a self-hosted runner
  with the image cached is the realistic shape). Whether the test project
  belongs in this repo as a fixture or is fetched. The NI container license
  permits automated testing and builds; the relink step in Step 2 exercises
  lvpm's own tooling on packages, which is the reading of "modifying
  LabVIEW code" to settle before that step runs on shared infrastructure.

## Housekeeping

- `Replace Mode = If Newer` compares timestamps instead of "write when absent".
- Read hook VIs' `error out` back.
- A failure part-way through unpacking leaves files behind with no manifest.
- Linux target detection exists but is untested.
