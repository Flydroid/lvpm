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

A project keeps its packages under `.lv-venv/` by default, mounted into LabVIEW as an
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

### Iteration 2 - WIP

- **Instance isolation — done.** A venv's LabVIEW gets its own
  compiled-object cache (`CompiledObjCacheRoot`), `LabVIEW Data`
  (`DefaultDataFileLocation`: autosave, `ExtraVILib`, crash reports) and
  `TEMP`/`TMP`, all under `.lvpm/`. Measured on 2025 and 2026: nothing lands
  in the user's folders, and a venv instance next to the primary IDE no
  longer shares its cache database, offers to recover its autosave files or
  writes into its log.
- **Seeding the venv's `LabVIEW Data`.** Today nothing is seeded: a venv
  starts empty, so personal customisations kept there (icon templates,
  glyphs, right-click plugins, custom probes, DQMH module templates, Quick
  Drop plugins) are missing in the venv IDE. Needs discussion before
  building:
  - Who decides what is seeded: the user (`config.toml`, personal tooling)
    or the project (`lvpm.toml`, shared team tooling), or both?
  - Copy at `venv create`, or refresh at every `lvpm launch`? A copy keeps
    the venv isolated but goes stale; a refresh overwrites what was changed
    inside the venv.
  - Is a folder the right unit? Some of these are better delivered as
    packages (dev-dependencies) than copied from one developer's machine.
- **The venv keeps its own ini — done.** A venv LabVIEW writes its options
  back to `.lvpm/labview.ini` when it closes (measured on 2026), so that file
  — copied from the target's ini by `venv create` — is now the base of every
  launch; only lvpm's keys are reapplied. Deleting it inherits the target's
  settings afresh at the next launch.
- **Install hooks in a venv.** Today none run. `ni_lib_advanced_http_client_api`
  genuinely needs its PostInstall (it repairs Call Library paths); DQMH did
  not (its palette came from the overlay). Candidate: run PostInstall inside
  the venv instance, snapshot the real installation before and after, move
  palette files it dropped into `<menus>` into the addon, warn about anything
  else. Per-dependency `hooks = "run" | "skip"` in the manifest makes it an
  explicit opt-in.
- **Relink what a partial run left behind.** A rerun after a failed install
  only relinks the packages _it_ copied; packages the first run copied stay
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

## The manifest and the lockfile - WIP

`lvpm.toml` (done: exact pins, `>=` floors, `*`, hosted `[sources]` and
`defaults = false`, `labview` minimum, `[nipm.dependencies]`). Local package
directories are user-level `sources.local` settings, not manifest sources.
Reference and `vipm.toml` comparison: [manifest.md](manifest.md).

- **`lvpm.lock`** — the resolved closure (name, version, source URL, MD5) so
  `lvpm install` restores the identical venv on another machine and only
  `lvpm update` re-resolves. The thing a venv without a lock only pretends to
  be.
- **`lvpm sync`** — after `lvpm.lock`: make the venv match the lock
  exactly, installing what is missing and uninstalling what the lock does not
  list. `install` stays additive. `--dry-run` shows both lists. Waits for the
  lock because today's resolver does not follow already-installed
  dependencies, so it cannot say what is still needed; notes in
  [manifest.md](manifest.md#to-do).
- **`lvpm add <pkg>[@constraint]` / `lvpm remove <pkg>`** — edit the manifest
  and install/uninstall in one step (the `toml` crate can write; the lockfile
  wants this too).
- **Constraint grammar (Cargo)** — bare and `^` caret, `~` tilde, `=`
  exact, `>=` floor, `*` newest, in every dependency table; `name@x` stays
  CLI-only. Breaking: a bare version is exact today. `vipm.toml` bare
  versions are read as `=x`.
- **`[dependencies.<name>]`** — done: named group tables shaped like
  `[dependencies]` with a reserved `include-groups = [...]` key, selected
  with `--with`, `--without`, `--only`; only `main` installs by default.
  Open: whether `dev` should install by default. Reference in
  [manifest.md](manifest.md#named-dependency-groups).
- Per-dependency tables: `{ version = "...", hooks = "run", source = "local" }`.
- `[venv]` overrides: `dir`, `vi-server-port`, `bitness`.
- **`vipm.toml` conversion** — done for `--manifest vipm.toml`
  (`src/vipm_interop.rs`: `labview-version`, exact `[dependencies]`,
  `[dev-dependencies]` as group `dev`, `[nipm.dependencies]`, a note naming
  what was not used). Not discovered; whether it should be is an open
  decision in [manifest.md](manifest.md#to-do). Mapping in
  [manifest.md](manifest.md#reading-a-vipmtoml-with-lvpm).

## Configuration

Done: `lvpm config set|get` persists `cache` and `sources.local` in the
user's `config.toml`; `LVPM_CONFIG_<KEY>` overrides either value. Local
package directories are global sources, read directly from disk and not yet
stored in the content cache.

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
  `oglib_*`); `resolvable()` accepts a path if _any_ ancestor directory exists,
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

## Building: `lvpm build`

Run a project's LabVIEW build specifications (EXE, packed library, LLB,
source distribution, zip, installer) the way `install` runs a relink: through
VI Server, headless where wanted, against the project's own dependencies.

- **A standard or custom `Build.vi`.** The standard one is embedded in the
  binary, like the relink VI: it opens the `.lvproj`, runs the named build
  specs and reports each step. A project can name its own `Build.vi`
  instead. No LabVIEWCLI: lvpm drives LabVIEW over its own VI Server client.
- **Arguments to the Pre-Build and Post-Build VIs:** version, target LabVIEW
  and bitness, output folder, commit.
- **`[build]` in `lvpm.toml`:** which build specs, where the version comes
  from (`project.version` or a git tag), the output folder.
- **Reproducible inputs.** The build runs in a LabVIEW started on the
  project's venv, with the versions from `lvpm.lock`, so another machine
  builds from the same packages.
- **Headless and in containers,** as install does today, so CI builds with
  no display.
- **A build matrix:** several LabVIEW versions and both bitnesses in one
  call.
- **Events** (see Terminal UI): build progress and errors reach the CLI, the
  TUI, a GUI and `--json` in one form.
- **Build, pack, publish.** A built packed library or source distribution
  becomes an lvpm package (the package format and PPL support above), its
  dependencies taken from the lockfile, and a publish step puts it on a
  feed. That closes the loop: build, package, publish, install.
- **Checks first:** the build spec exists, the LabVIEW version and its
  Application Builder are installed, the output folder can be written.

To measure before building on it: whether NI's container images include
Application Builder; how the build spec API behaves headless, on Windows and
Linux; whether a packed library built in a venv links to `<vilib>` the same
way as one built against a global install.

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

## Terminal UI

`lvpm tui` (or `lvpm` with no arguments): a full-screen front end over the
commands that exist, with no logic of its own. The main reason is SSH: a test
stand, a lab PC, a headless Linux build machine or a container
(`docker exec -it <name> lvpm tui`) gets a package manager with no display,
RDP or VNC. Windows has an OpenSSH server, so the same applies there.

Views:

- **Browse** — every package in the configured sources, filtered by name,
  vendor or category, and by installed, updates or the selected LabVIEW.
  Details: description, all versions, dependencies, license, file count.
  `v` picks an exact version (`name@version`).
- **Installed** — version, relinked or not, hook results, newer versions.
  Install, update, uninstall, relink and rerun hooks, one key each.
- **Plan** — before anything changes: packages added, upgraded or
  downgraded, file count, relink folders, hooks; the `--dry-run` output with
  structure. Downgrades need a confirmation, as `--allow-downgrade` does.
- **Progress** — one row per phase (resolve, download, unpack, relink per
  folder, hooks) and the live log. `Esc` cancels between folders.
- **Project** — edit `lvpm.toml` dependencies (pin, `>=` floor, `*`), create
  and launch the venv.
- **Targets** — LabVIEW versions, VI Server port and state, start.
- **Sources** — add or remove sources, refresh the indexes.

For SSH:

- Works with no display: shows that LabVIEW runs headless and which steps
  that turns off (relink and hooks by default).
- A dropped connection must not kill a relink: the job runs detached and the
  TUI attaches to its log again, or the TUI runs inside `tmux`.
- Redraw only what changed; no animation.
- No Nerd Font glyphs, no dependence on true colour, an ASCII fallback for
  box lines.

How: the refactor and the event layer below first, then the TUI on them.

1. **Read-only views:** Browse and Installed, on `ratatui` and `crossterm`
   (pure Rust: the static musl and Windows builds stay as they are), behind a
   cargo feature.
2. **Install and uninstall** with Plan and Progress; relink and hooks on a
   worker thread.
3. **Project, Targets and Sources.**

What a terminal cannot do: package icons and screenshots, rich release
notes, drag and drop. A GUI (`egui`, or `tauri`) can follow on the event
layer if Windows users ask for one; the TUI does not block it.

### Before the TUI: split `main.rs`

`main.rs` holds 1,856 lines and 130 `print!` calls; `cmd_install` alone is
412 lines of resolution, download, unpack, hooks, relink and output.
[refactor-main.md](refactor-main.md) is the plan: the target layout (`cmd/`
for the CLI side, library modules that never print), the order of work from
pure moves to the event layer, and which open branches land before the
moves. The CLI changes that ride on it are in [cli.md](cli.md).

### Event layer and JSON interface

Operations stop printing. They take a sink and send events; a front end is
whatever implements the sink. One sender and one listener per operation, so
a channel or a callback is enough; no broker.

```rust
enum Event {
    Plan(Plan),                                   // what will change
    Phase { name: Phase, state: State },          // resolve, download, unpack, relink, hooks
    Progress { item: String, done: u64, total: u64 },
    Log { level: Level, text: String },
    Done(Result<Summary, ErrorReport>),
}
trait Sink { fn emit(&self, e: Event); }

fn install(ctx: &Context, plan: &Plan, sink: &dyn Sink, cancel: &Cancel) -> Result<Summary>;
```

| Front end | Sink |
|---|---|
| CLI | prints each event as text, as today |
| TUI, in-process GUI (`egui`, `tauri`) | a channel to the UI thread |
| `--json` | one JSON object per line on stdout; CI scripts read this, not the text |
| `lvpm serve` (later) | the same JSON over a local socket |

Commands go the other way: install, uninstall, relink, cancel. Cancel is a
shared flag the operations check between steps (between relink folders,
between hooks).

`--json` and `serve` make any language a front end, LabVIEW included: a GUI
in LabVIEW reads the events with its own JSON tools, so lvpm can sit in the
IDE's Tools menu while the package logic stays in Rust. That makes the
events an interface: each message carries `"schema": 1`, fields are only
added within a schema version, and a removal or change of meaning raises it.

### Concurrent callers

With a TUI, a GUI and the CLI, two callers can act on one target at once.
Today nothing stops them: install records are written in place
(`fs::write`), and two installs into one `vi.lib` interleave. The index
cache is already safe (SQLite in WAL mode, downloads renamed into place).

- **One writer per target.** Every operation that changes a target
  (install, uninstall, relink, run-hooks, refresh, venv create or remove)
  takes an exclusive lock on the target's record directory
  (`%ProgramData%\lvpm\<target>`, `/var/lib/lvpm/<target>`, or the venv's
  `.lvpm`). A file lock (`LockFileEx`, `flock`), so the OS releases it when
  the process dies and no stale lock is left. Different targets, and
  different venvs, run in parallel.
- **Readers take no lock.** `list`, `search` and the Browse view read
  records that are written to a temporary file and renamed into place, so a
  reader sees the old record or the new one, never half of one.
- **The second writer waits or stops.** The lock file holds who holds it
  (process id, command, start time). The CLI waits with a message ("waiting
  for `lvpm install` (pid 4711, started 10:02) on LabVIEW 2026"), and
  `--no-wait` makes it fail at once with that message. The TUI or GUI shows
  the target as busy and disables the actions that write.
- **Plans are checked again.** A plan made before the lock is taken can be
  stale. After taking the lock, the operation compares the records with what
  the plan assumed; when they differ it stops and the front end resolves
  again.
- **One LabVIEW per port.** Relink and hooks drive the LabVIEW on the
  target's VI Server port. The target lock already serialises lvpm's own
  use of it; a user working in that LabVIEW at the same time is not
  something a lock can prevent, and is reported, not handled.
- **With `lvpm serve`** the daemon holds the lock and queues requests, so
  GUIs see a queue instead of a refusal; a CLI run beside it still takes
  the same file lock, so the two never collide.

## Housekeeping

- `Replace Mode = If Newer` compares timestamps instead of "write when absent".
- Read hook VIs' `error out` back.
- A failure part-way through unpacking leaves files behind with no manifest.
- Linux target detection exists but is untested.
