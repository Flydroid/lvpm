---
name: linkfix-tools
description: Debug lvpm's linkfix and venv installs with the Python tools in tools/linkfix/ — what linkfix or a LabVIEW save changed in a package (inventory, linker_diff, linkobjs, qualcheck, deptree), whether packages are venv-ready before install (audit, predict_dirty, deps), why a venv LabVIEW crashed (crashdump), and the crash-bisect loop that swaps one package at a time into a venv. Use for "what did linkfix change", "why does LabVIEW crash / ask to save in the venv", "compare this VI with the packaged one", "bisect the crash", "check the qualifiers", "audit lvpm.toml", or reading a VI Server log or capture.
---

# linkfix and venv debugging tools

The tools are plain Python 3.11+ scripts in `tools/linkfix/`, run from the
repo root (the two VI Server tools sit one level up, in `tools/`). None of
them needs LabVIEW or VIPM; only the probe VIs in `tools/linkfix/probes/` do. They read lvpm's own data through
`tools/linkfix/lvpmdata.py`:

- **Install records:** `<venv>/.lvpm/installed/<package>.json` give each
  installed file's absolute path.
- **Package cache:** `%LOCALAPPDATA%\lvpm\cache\lvpm-cache.db` plus the
  content store hold the original archives.
- **Spec:** the `spec` file inside each archive.

`linkobjs.py` re-implements the link-object walk of `src/linkfix.rs`. When
the two disagree, the Rust code is right and the Python needs updating.

## Before you start

- **Archives must be in lvpm's cache.** A tool that compares against the
  packaged bytes says "not in lvpm's cache" otherwise. Import first:
  `target/debug/lvpm.exe cache add <file.vip or folder>`. A folder imports
  every archive in it.
- **`WHERE` arguments** take a venv folder or a project that has one. Venvs
  made before the default moved to `.lv-venv` live in `.project`, so set
  `LVPM_CONFIG_VENV_DIR=.project` (PowerShell: `$env:LVPM_CONFIG_VENV_DIR='.project'`)
  for both the tools and lvpm.
- **Copied venvs:** a venv copied folder-for-folder keeps install records that
  name the original venv's paths. `lvpmdata.installed()` rebases them onto the
  venv you pass, so the tools compare the right files.
- **`crashdump.py`** needs `pip install minidump`. `predict_dirty.py` needs
  `pip install pylabview`.

## Which tool answers which question

| Question | Command |
|---|---|
| Which files in a package differ from the packaged bytes? | `python tools/linkfix/inventory.py vs-archive <WHERE> [pkg ...] --list` |
| What changed between two moments (before/after linkfix, before/after a save)? | `inventory.py snapshot <WHERE> -o a.json`, then again `-o b.json`, then `inventory.py diff a.json b.json` |
| Which resource blocks changed in those files? | `python tools/linkfix/linker_diff.py <WHERE> <pkg>` (`--strings "<file substring>"` for the strings added and removed) |
| Which link objects changed, and how? | `python tools/linkfix/linkobjs.py show "<vi>"` on both copies and compare: ident, qualifier, path type (`tp=0` absolute/token, `tp=1` relative), path, tail length |
| Are the qualifier chains right? | `python tools/linkfix/qualcheck.py <venv store folder> [pkg filter]` (the store is the folder holding the package folders, for example `C:\Git\venv-check\.project`) |
| Do a tree's declared dependencies resolve? Compare two trees by their dependency sets, not bytes? | `python tools/linkfix/deptree.py check <tree>` / `deptree.py compare <a> <b>` / `deptree.py show "<vi>"` |
| Are packages venv-ready (embedded compiled code, absolute or missing link paths)? | `python tools/linkfix/audit.py --manifest lvpm.toml` or `--installed <WHERE>` or package names; `--show absolute\|missing\|relative` |
| Which VIs will LabVIEW want to save after loading? | `python tools/linkfix/predict_dirty.py --manifest lvpm.toml [--observed <venv store>]` |
| Does LabVIEW still want to save this (linkfixed) VI? (needs LabVIEW) | `Save Buffer Probe.vi`, see "LabVIEW probes" below |
| What is a package's dependency closure, and which packages carry install hooks? | `python tools/linkfix/deps.py <pkg>[-version]` |
| Why did a venv LabVIEW crash, and is it the same crash as before? | `python tools/linkfix/crashdump.py --venv <WHERE>` or `crashdump.py <file.dmp>` |
| What did a VI Server client send or receive? | `python tools/parse_viserver_log.py <log>` / `python tools/decode_viserver_pcap.py <cap.pcapng> --port <port>` |

## LabVIEW probes

`tools/linkfix/probes/` holds two VIs that ask a running LabVIEW directly.
Each `.vi` comes with the `.xml` it was generated from. Run them in the venv's
own LabVIEW with `lvpm vi-run`:

- **`Save Buffer Probe.vi`** gets the bytes LabVIEW would write if it saved the
  VI (`Save.To Buffer`), without saving anything, and writes them to
  `out path`. Compare that buffer's link tables with the file on disk
  (`linkobjs.py show` on both): if they match, LabVIEW has nothing left to
  save, which is what linkfix aims for. This is the strongest oracle for a
  linkfixed file.
  ```
  target/debug/lvpm.exe --project <venv> vi-run tools/linkfix/probes/"Save Buffer Probe.vi" --set "vi path=str:<vi>" --set "out path=str:<buffer file>" --get status --get "error out"
  ```
- **`Dirty Probe.vi`** reads a VI's modification bitsets (VI, block diagram,
  front panel) and its User Changes flag. Don't trust a clean result: the
  bitsets do not see LabVIEW relinking a VI as it loads, so a VI can read
  clean and still ask to be saved on close. Use `Save Buffer Probe.vi` to
  decide.

## Workflow: what did linkfix do to a package?

1. `inventory.py vs-archive <WHERE> <pkg> --list` lists the rewritten files.
2. `linker_diff.py <WHERE> <pkg>`: linkfix should only ever change `LIvi`,
   `LIbd`, `LIfp` and `LIds`. Any other changed block, or a moved save
   version, means LabVIEW saved the file, not linkfix.
3. Run `linkobjs.py show` on the packaged copy (from a venv that was never
   linkfixed, or extracted from the archive) and on the fixed copy. Name the
   objects that changed.
4. Find an oracle: a copy LabVIEW saved itself, for example the same package
   in a real LabVIEW's `vi.lib` after a VIPM mass compile. `linkobjs.py show`
   on it shows what LabVIEW writes. The oracle's save version can differ from
   the packaged file's, so compare path forms, not tails. With LabVIEW at hand,
   `Save Buffer Probe.vi` on the fixed file gives an oracle in the file's own
   context.

## Workflow: bisect a venv crash

The method: start from a venv that works without linkfix, swap in one
package at a time in its linkfixed form, and launch until it crashes.

1. **Get a fixed package without touching the bisect venv.** Copy the
   unfixed store to a scratch folder (robocopy `/E /XD .lvpm`), then run
   `target/debug/lvpm.exe linkfix --tree <scratch store>` on the copy.
2. **Swap only the package under test** into the bisect venv:
   `robocopy <scratch>\<pkg> <venv store>\<pkg> /MIR`. Robocopy's exit code
   1 means files were copied. Hash one changed file to confirm the swap.
3. **Clear the venv's compile cache and autosave:** delete
   `<store>\.lvpm\objcache` and `<store>\.lvpm\data\LVAutoSave`. Delete each
   folder by its literal path; a wildcard delete is refused.
4. **Launch with the debug ini keys.** Only one LabVIEW may run per venv.
   ```
   target/debug/lvpm.exe --project <venv> launch --ini NIER=True --ini DPrintfLogging=True --ini LVdebugKeys=True --ini debugging=True <project.lvproj>
   ```
   The user then opens the VIs that crash.
5. **Read the result.** Run `crashdump.py --venv <venv>` **right away**:
   - A ~16 MB dump with the same repeating frames is the same crash.
   - 1–4 MB dumps are warnings, not crashes.
   - No new dump, and `<store>\.lvpm\labview.ini` rewritten at close, means
     LabVIEW closed cleanly. A venv LabVIEW writes its ini back only on a
     clean exit.
   - The user saying "it closed" isn't enough: check the dump.
6. **Narrow inside a package** once a package is guilty. Diff it (workflow
   above), then make a linkfix build that skips one kind of change, re-fix,
   swap, launch.

## Gotchas

- **Save crash dumps immediately.** NI's error reporter removes
  `reports\*.dmp` on a later LabVIEW start (seen 2026-10-21: both bisect
  dumps vanished). Copy the dump out, or run `crashdump.py`, before the next
  launch.
- **Dump location:** a venv LabVIEW writes reports under
  `<store>\.lvpm\data\LVInternalReports\LabVIEW\<version>\`, not the user's
  `Documents\LabVIEW Data`.
- **Two LabVIEW versions:** a LabVIEW update between rounds shifts the
  `LabVIEW.exe` and `mgcore` offsets slightly. The `dfir_*.dll` offsets have
  stayed identical, so compare on those.
- **Missing packages:** packages missing from every venv being compared (not
  just one) cannot explain a difference between them. Leave them out until
  the bisect is done, so each round changes one thing.
- **Byte diffs of relinked VIs never converge**, because LabVIEW reorders
  `LIfp` entries. Compare dependency sets (`deptree.py compare`) or link
  objects (`linkobjs.py`), not hashes, when LabVIEW did the save.
- **`lvpm-cache.db` missing:** the machine still has the old index cache.
  Any `lvpm cache add` creates the database.
