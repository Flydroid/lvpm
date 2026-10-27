# lvpm-tui in real terminals: the manual checklist

The acceptance tests draw lvpm-tui on a test backend. They cannot show what a
real terminal does with raw mode, the alternate screen, a resize, or the
restore on exit. Run this checklist in each terminal below whenever a slice's
plan step says so, and copy the result table into the pull request.

## Before you start

No release ships `lvpm-tui.exe` yet, so build it from source. Check out the
newest TUI branch you want to test; each one contains the slices before it.

```bash
cargo build -p lvpm-tui -p lvpm
```

`lvpm-tui` takes no flags beyond `--help` and `--version`. It reads the same
settings and indexes as `lvpm`, and it picks its destination the way `lvpm`
does with no flags: the nearest venv at or above the current directory, else
the newest LabVIEW that `lvpm targets` lists. The header shows the destination.

`i` installs into that destination. To keep your LabVIEW installation as it
is, start `lvpm-tui` inside a scratch project that has a venv:

```bash
mkdir %TEMP%\tui-test
cd %TEMP%\tui-test
(echo [project]& echo name = "tui-test"& echo version = "0.1.0"& echo labview = "2026") > lvpm.toml
<repo>\target\debug\lvpm.exe venv create
<repo>\target\debug\lvpm-tui.exe
```

Replace `<repo>` with the repository root and `2026` with a LabVIEW year that
`lvpm targets` lists. Remove the venv afterwards with `lvpm venv remove`.
Checks 1 to 17 do not install, so you can also run them from the repository
root.

Open each terminal in the repository root:

- **Windows Terminal**: `wt -d .`
- **conhost** (the classic console): `conhost.exe cmd.exe /k cd /d %CD%`

Before you start lvpm-tui, type `echo before` so that the screen has a line
you can recognize.

## Checks

| # | Do this | Expect this |
|---|---------|-------------|
| 1 | Make the window at least 80 columns by 24 rows. Run `target\debug\lvpm-tui.exe`. | The `echo before` line is gone. The top row shows ` lvpm ` in reverse video and the destination. A bordered box fills the middle. The bottom row shows `/ search`, `f filter`, `R reload feeds` and `q quit`. With no LabVIEW installed, it also says install needs a LabVIEW or --prefix. |
| 2 | Make the window narrower than 80 columns. | Only the message "The terminal is WxH; lvpm-tui needs at least 80x24." shows, with the current size. No border, no header. |
| 3 | Make the window wider again, back to at least 80x24. | The header, the box and the footer from check 1 come back. |
| 4 | Do checks 2 and 3 again, but make the window shorter than 24 rows. | The same as checks 2 and 3. |
| 5 | Press `q`. | lvpm-tui exits. The `echo before` line is back. The cursor shows. Type `abc`: the letters echo, and Enter runs the line. |
| 6 | Start lvpm-tui again and press Ctrl-C. | The same as check 5. |
| 7 | Start lvpm-tui again and wait for the catalog. | "Loading the catalog…" shows first, with one line per feed that says it is being checked or downloaded and then how it answered. Then a table with the columns Package, Source, Installed, Available and Status; Source names a feed by its host (`github` for GitHub), else `local` or `cache`. The selected row is in reverse video with `>` before it. |
| 8 | Press Down, Up, PgDn, PgUp, End and Home. | The selection moves; the list scrolls so that the selected row stays visible. |
| 9 | Press `/`, type part of a package name, then press Enter. | The `Search:` line shows the text and a `_` cursor while typing. The list shrinks to the matching packages. |
| 10 | Press `/`, then Esc. | The search is cleared and every package is listed again. |
| 11 | Press `f` three times. | The filter goes `installed`, `updates`, `all`. |
| 12 | Select a package and press `R`. | The header says `reloading…` while the list stays usable, then the list again with the same package selected. |
| 13 | Select a package whose status is `needs LV …` and press `i`. | The bottom row starts with "<package> needs LabVIEW <year>". The next key clears it. |
| 14 | Select a package and press Enter. | A `Versions` pane on the left, newest first, with `>` on the newest. On the right, the package's fields, then "Reading the details of …", then its description, vendor, license and files. The bottom row shows `Tab versions/fields`, `PgDn page down`, `Esc back` and `q quit`. |
| 15 | Press Down, then Tab, then PgDn and PgUp. | Down picks the next version and its fields show. After Tab the fields pane's title is bold; PgDn and PgUp scroll the fields, and the picked version stays. |
| 16 | Press Esc. | The list is back with the same package selected. |
| 17 | Disconnect the network and press `R`. | The list stays. A reverse-video banner says reloading failed and the list shown is from the cache, and names each feed. |

## Result table

Copy this into the pull request, with one mark per cell: ✅ passed, ❌ failed
(add a note), or — not run.

| Check | Windows Terminal | conhost |
|-------|------------------|---------|
| 1 start | | |
| 2 too narrow | | |
| 3 back to 80x24 | | |
| 4 too short and back | | |
| 5 quit with q | | |
| 6 quit with Ctrl-C | | |
| 7 catalog loads | | |
| 8 navigation | | |
| 9 search | | |
| 10 clear search | | |
| 11 filter | | |
| 12 reload | | |
| 13 gated install | | |
| 14 details | | |
| 15 versions and scrolling | | |
| 16 back to the list | | |
| 17 reload offline | | |
