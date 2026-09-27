# LabVIEW on Linux

lvpm builds and runs on Linux, and drives LabVIEW for Linux the way it drives
the Windows IDE: files copied, then relinked and hooks run over VI Server.
Everything below was measured on 2026-09-26 against NI's container image
`nationalinstruments/labview:2026q3-linux` (LabVIEW 2026 Q3, Ubuntu 22.04).
[scripts/linux-e2e.sh](../scripts/linux-e2e.sh) repeats every check in a
throwaway container in about a minute and a half, and [CI](#ci) runs it.

## Building

```console
$ cargo build --release                  # target/release/lvpm, for this machine's glibc
$ cargo test
$ rustup target add x86_64-unknown-linux-musl
$ CC_x86_64_unknown_linux_musl=musl-gcc cargo build --release --target x86_64-unknown-linux-musl
```

Nothing in the dependency tree needs OpenSSL: reqwest's `default-tls` is
rustls on aws-lc, which needs only a C compiler (`musl-gcc` for the static
build). A binary built on a current distribution does not start in NI's image
(`GLIBC_2.38 not found`; the image has 2.35), so the static musl build is the
one to ship.

### Windows stays as it was

The platform is chosen when lvpm is compiled, not when it runs: each
difference below is a `#[cfg(windows)]` / `#[cfg(target_os = "linux")]`
pair, or `#[cfg(not(windows))]` where the Linux half holds on any POSIX
system (separators, line ends, the per-user cache), and the Windows half is
the code that was there before. Other platforms do not build (see the
roadmap's macOS section). What a Windows user can
see changed is small: the "VI Server is disabled" and "no LabVIEW.exe"
messages name the full path of the file, and a `Target Dir` such as
`<vi.lib>/addons/Foo` now expands to `...\vi.lib\addons\Foo` instead of
`...\vi.lib\addons/Foo` (the same path either way). Tests that pin
Windows behaviour are `cfg(windows)`, with Linux counterparts, and run on a
Windows runner in [CI](#ci).

## What differs

| | Windows | Linux, measured | lvpm on Linux |
|---|---|---|---|
| Installation | registry; `LabVIEW.exe` | `/usr/local/natinst/LabVIEW-<year>-64`, Q1 and Q3 of a year sharing it; `/etc/natinst/labview-<year>-64` links to its `etc`, whose `labview.dir` names it. `labview` in it is a symlink to the edition's binary (`labviewprofull`) | reads `/etc/natinst`, scans `/usr/local/natinst`, requires `labview` |
| Internal version | registry key (`26.3`) | not in the directory name; `readme/UNINSTALL` in the install sets `LV_MAJOR_VER=26`, `LV_MINOR_VER=3` | reads it; the year when it is missing |
| Preferences | `LabVIEW.ini` beside the exe | `~/natinst/.config/LabVIEW-<year>/<name>.conf`, per user, named after the name LabVIEW was started as (`labview.conf` for `labview`). Absent until the user's first launch; LabVIEW adds the `[LabVIEW]` header and writes LF | reads `labview.conf`; a venv ini starts from `[LabVIEW]` when there is none |
| VI Server default | on | **off**: without `server.tcp.enabled=True` LabVIEW never listens. NI's image seeds all three `.conf` files with it | absent means off, and the error names the file, instead of a 120 s wait |
| VI Server port | 3363, 3364 on 2026 | 3363 on 2026 Q3 | read from the `.conf` |
| Protocol | | unchanged: handshake, error 63 until initialised, open, run, save, control values | |
| Paths on the wire (`PTH0`) | drive letter first | no drive and no root component: `/usr/local` is `usr`, `local` | decoded to `/usr/local/...`, not `usr\local\...` |
| Relink folder | `C:\...` | `/...`. Handed the backslashed form, the relink VI walks nothing and reports success: 0 items where the same folder gives 2 | sent as it is |
| Palette and menu refresh | two shipping VIs | the same VIs at the same relative paths | joined a component at a time (a `\` inside one `join` separates only on Windows) |
| `<OS …>` tokens | environment variables | what `Get System Directory.vi` answers (below) | the same table |
| Records of a global install | `%ProgramData%\lvpm` | `/var/lib/lvpm/<target>/` | (was a `C:\ProgramData` directory under the working directory) |
| Index cache | `%LOCALAPPDATA%\lvpm\cache` | `$XDG_CACHE_HOME/lvpm`, else `~/.cache/lvpm` | |
| Relink VI, staged PostUninstall | `%TEMP%` | `/tmp` is shared, and a name another user wrote first is one this user cannot write | the per-user cache |
| Target Dir tails | either separator | `<vi.lib>\addons\Foo` would be one directory named `addons\Foo` | split on both |
| Headless | `LV_RTE_HEADLESS=1` (2026 and later) | the same switch, also from 2026. LabVIEW starts its own `Xvfb :99` and needs no `DISPLAY`; the log is `/tmp/labview_<version>_headless_<user>_cur.txt` | the hint names it |
| `-pref <ini>` | yes | yes: a second instance on its own ini and port runs beside the first and shares its Xvfb. The Linux binary has no `AllowMultipleInstances` key | unchanged |
| LVAddons | `C:\Program Files\NI\LVAddons` | `/usr/local/natinst/share/lvaddons` (NI's VI Analyzer support ships there). `LVAddons.AdditionalLocations` is honoured | the venv path goes with forward slashes |
| Restarting on a port | immediate | **a LabVIEW started while its port holds a TIME_WAIT connection (up to 60 s after the last one closed) never serves VI Server**, and never retries | waits for the port, 65 s at most, before starting LabVIEW |
| Ctrl+C | IDE detached | a child in lvpm's process group gets the terminal's SIGINT | LabVIEW gets a process group of its own |

### `<OS …>` tokens

The tokens are the directory types of LabVIEW's `Get System Directory.vi` by
name. lvpm resolves them to what that VI returned over VI Server on Linux, run
as root:

| Token | Linux |
|---|---|
| `<OS User Documents>` | `$HOME/Documents` |
| `<OS User Desktop>` | `$HOME/Desktop` |
| `<OS User Application Data>` | `$HOME` |
| `<OS Public Documents>`, `<OS Public Application Data>`, `<OS Application Files>` | `/usr/local` |
| `<OS System Core Libraries>` | `/usr/lib` |
| `<OS Boot Volume Root>` | `/` |
| `<temp>` | `/tmp` |

The VI's remaining types, for reference: User Preferences `$HOME`, Public
Preferences `/etc`, System Installed Libraries `/usr/local/lib`, Public Cache
`/var/cache`.

### LabVIEWCLI

LabVIEWCLI on Linux wants `-LabVIEWPath`, the operation name first and
`-Headless` last; given `-Headless` in between, it prints its usage and exits
0. A clean mass compile says `MassCompile operation succeeded` and exits 0; a
broken VI shows as `### Bad VI:` in the log, with exit code 3.

## Verified

With `scripts/linux-e2e.sh`, all in one container:

- `lvpm targets` reports `LabVIEW 2026 (64-bit) v26.3`.
- A headless `lvpm install --relink --hooks` in a project with no venv starts
  LabVIEW, runs PreInstall, copies into `vi.lib` and `$HOME/Documents`,
  relinks and runs PostInstall. Records land in `/var/lib/lvpm`.
- `list`, `refresh` (both VIs run), `relink --all`, `run-hooks` and
  `uninstall --all` work, and the uninstall prunes what it created.
- `vi-save` rewrites a LabVIEW 2020 VI as 26.3. A caller whose subVI moved is
  found, relinked and saved with a `<vilib>` link.
- In a venv: `venv create`, then `install --relink` in a LabVIEW started with
  `-pref` on the venv's port, then `launch` straight after. The overlay works:
  a VI linked to `<vilib>/lvpm_linux_test/...` runs on the venv's LabVIEW and
  is broken (error 1003) on a fresh plain one.

By hand: as a non-root user with no `.conf`, `lvpm start` stops at once with
the "VI Server is disabled" message, and the whole venv flow works without
root.

## CI

[.github/workflows/build.yml](../.github/workflows/build.yml), on every push
to any branch, on pull requests from forks, and by hand. A newer push to a
branch other than `main` cancels its run still in progress.

- **build** — one job per platform on its own runner: `windows-latest`
  (`x86_64-pc-windows-msvc`) and `ubuntu-latest` (static
  `x86_64-unknown-linux-musl`). Each runs `cargo test` and a release build,
  and uploads its executable.
- **collect** — puts both executables in one artifact,
  `lvpm-<version>-<commit>`, for handing to testers.
- **linux-e2e** — `scripts/linux-e2e.sh` with the Linux executable, in
  `nationalinstruments/labview:2026q3-linux` (pinned).
- **linux-packages** — `scripts/linux-packages.sh` in the same image, with
  public packages that are plain G and need no NI driver (eleven OpenG
  libraries and the JKI State Machine, with their dependencies): a headless
  install that must start no LabVIEW, a LabVIEWCLI mass compile of every
  folder they own that must report no bad VI, `relink --all`, an uninstall
  that must leave the LabVIEW tree as it was, and the same set relinked in a
  venv. Its logs are uploaded.

## Not done

What Linux still needs, most important first.

1. **OS gates on packages.** lvpm ignores `Exclusive_OS`, both in the index
   (`Platform.Exclusive_OS`) and per file group, so on Linux it would install
   a package that declares itself Windows-only. LabVIEW 2026 for Linux ships
   NI's Wine layer (`ni-wine`, `ni-wine-dotnet-runtime-80`,
   `ni-dotnetcore-interface`); what that means for .NET nodes is not measured.
2. **Root.** The installation is owned by root. A global install needs write
   access to it, and the relink and hooks run in a LabVIEW that has to save
   into it. Under `sudo`, Ubuntu sets `$HOME` to root's, so lvpm reads root's
   `labview.conf` and starts LabVIEW as root. A venv needs neither. lvpm should
   check write access up front and say so.
3. **Case.** Linux is case-sensitive; much of what lvpm and packages were
   written against is not. `install.rs` looks a manifest up by the name as
   typed, so `uninstall OGLib_Error` misses. Archive entries are matched
   case-insensitively but written in the spec's case. `venv::port_for`
   lowercases the path, so two repos that differ only in case share a port.
   And a VI that links a subVI in a different case does not load on Linux.
4. **Releases.** CI builds both executables; a tagged release still carries
   only Windows, and there is no `install.sh` to match `install.ps1`.
5. **Not measured.** LabVIEW for Linux before 2026 (the `.conf` directory
   name, `readme/UNINSTALL`, `LVAddons.AdditionalLocations` from 2024 Q1 on);
   other distributions (RHEL, openSUSE); Community edition; an interactive IDE
   on a real display. Headless mode arrived with LabVIEW 2026: the 2025 Q3
   binary has no `LV_RTE_HEADLESS` and no `--headless` (its image sets
   `EnableCICDFeaturesForLabVIEW` instead), so lvpm cannot tell that a 2025
   machine is headless.
6. **Containers.** Start the container with `--init`. A LabVIEW that lvpm
   started is reparented to PID 1, and `sleep infinity` never reaps it. The
   TIME_WAIT rule applies to anything that restarts LabVIEW on a port within a
   minute, LabVIEWCLI included.
