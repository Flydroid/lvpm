# The command-line interface

> **This fork (2026-10-30).** Came in with the upstream merge and describes
> upstream's CLI at `39a5aaf`. This fork's command reference is
> [usage.md](usage.md); the `main.rs` split it mentions is done here as ADR 5.

What `lvpm`'s interface is today, what has to change before it is called
stable, and what is still missing. The [command reference](usage.md#commands) is
the user reference; this file is for deciding. The code changes ride on the
`main.rs` split in [refactor-main.md](refactor-main.md).

State as of `main` at `39a5aaf` (0.2.1).

## What "stable" will promise

From the first stable release, these change only with a major version:

- command and subcommand names, flag names and their meaning;
- exit codes;
- the `--json` output, by its `"schema"` number (see the roadmap's
  *Event layer and JSON interface*);
- config keys, `LVPM_CONFIG_*` variables and `lvpm.toml` keys.

Text output is not part of the promise. Scripts read `--json`.

Commands or flags marked hidden or under `debug` are not covered.

## Today

```text
lvpm [--labview-version YYYY] [--prefix DIR | --project DIR] [--global] [--refresh] <command>

  targets                       detected LabVIEW installations
  search <query>                search the indexes
  list                          what lvpm installed into the target
  install [<pkg>[@ver]] [--manifest FILE] [--dry-run] [--no-deps]
          [--no-relink | --relink] [--hooks] [--allow-downgrade]
          [--relink-timeout SECS] [--relink-progress NAME] [--keep-open]
  uninstall <pkg> | --all
  relink <pkg> | --all          [--relink-timeout] [--relink-progress] [--keep-open]
  run-hooks <pkg> | --all       [--timeout SECS]
  refresh                       [--timeout SECS]
  start                         [--wait SECS]
  launch [<lvproj>]
  venv create | status | remove [-y]
  config get <key> | set <key> [<value>...]
  cache add <path>
  app-probe <id>...             } VI Server primitives,
  vi-probe <vi>                 } for diagnosis
  vi-save <vi>                  }
  vi-run <vi> [--set N=T:V]... [--get N]... [--get-all] [--timeout] [--watch N] [--poll-ms MS] [--done N]
```

What works and stays:

- Names users already know from cargo and npm: `install`, `uninstall`,
  `list`, `search`, `config get|set`, `cache add`.
- No activation step. A venv at or above the working directory is used;
  `--global`, `--prefix` and `--project` override it.
- `<pkg> | --all` for every bulk command.
- `RelinkArgs` shared by `install` and `relink`, so the two cannot drift.

## Changes before freezing

Each item says what changes and whether existing scripts break. All are
proposals until the pull request that makes them.

### 1. Every command resolves its destination the same way

Today `install`, `uninstall`, `list`, `relink`, `launch` and `vi-*` use the
venv when there is one (`destination()`), but `start`, `refresh`,
`run-hooks` and `app-probe` only look at `--labview-version` / `--prefix`
(`roots_for()`). So inside a project, `lvpm start` starts the global
LabVIEW while `lvpm launch` starts the venv's, and `lvpm refresh` without a
flag fails.

Proposed: every command that touches a target goes through one
`Context` (`context.rs`). Where a command cannot act on a venv, it says so
and names the alternative instead of failing on a missing flag. `run-hooks`
in a venv refuses until hooks run in venvs (roadmap, *Install hooks in a
venv*). Breaking: no; commands that failed now work.

### 2. `--refresh` and `refresh` mean different things

The global flag `--refresh` revalidates the index cache; the command
`refresh` rebuilds palettes and menus.

Proposed: rename the flag to `--update-index`, keep `--refresh` as a hidden
alias until 1.0. Breaking: no, through the alias.

### 3. `--labview-version` is described as an install flag

It selects the LabVIEW for every command (`venv create`, `start`,
`launch`, `vi-*`), but its help says "Install into this LabVIEW version".

Proposed: help text "The LabVIEW to use, e.g. 2026"; add the short alias
`--lv`. Settle the manifest key at the same time: `lvpm.toml` says
`labview = "2025"` while the `venv create` help says "the manifest's
`labview-version`". Keep `labview` and fix the help. Breaking: no.

### 4. Debug commands out of the top level

`app-probe`, `vi-probe`, `vi-save` and `vi-run` are a third of the
top-level commands and are for diagnosing lvpm itself.

Proposed: `lvpm debug app-probe | vi-probe | vi-save | vi-run`, excluded from
the stability promise. The old names stay as hidden aliases for one minor
release. Breaking: only for scripts that use them, through the aliases not
at once.

### 5. `start` and `launch`

`start` starts the target's LabVIEW and waits for VI Server; `launch`
starts the venv's and can open a project. Once item 1 lands, both act on the
same destination.

Proposed: one command, `launch [<lvproj>] [--wait SECS]`. With `--wait` it
blocks until VI Server answers, which is what `start` does. `start` stays
as a hidden alias. Breaking: no.

### 6. Relink and hooks: one flag each, three values

`install` has `--no-relink`, `--relink` and `--hooks`, and the default of
each depends on `LV_RTE_HEADLESS`.

Proposed: `--relink auto|always|never` and `--hooks auto|always|never`,
`auto` being today's behaviour. `--no-relink` stays as an alias for
`--relink never`. Breaking: `--relink` and `--hooks` without a value would
need a value; clap's `default_missing_value = "always"` keeps them working.

### 7. Timeouts named one way

Waiting is `--wait` on `start`, `--timeout` on `run-hooks`, `refresh` and
`vi-run`, `--relink-timeout` on `install` and `relink`; `--poll-ms` is the
only flag with a unit in its name.

Proposed: `--timeout SECS` wherever one step is bounded,
`--relink-timeout` where the command has other timeouts too (`install`).
`start --wait` survives as `launch --wait` (item 5), which is about waiting
for something to come up, not about giving up. Breaking: `start --wait` is
aliased; the rest is unchanged.

### 8. `config unset`

`config set sources.local` with no values clears the list. One forgotten
argument empties the setting.

Proposed: `config set` requires at least one value; `config unset <key>`
clears. Add `config list` for every key with its value and where it came
from (environment, file, default). Breaking: `config set <key>` with no
value now fails with a message pointing to `unset`.

### 9. Confirmation

`venv remove` asks unless `-y`; `uninstall --all` removes everything
without asking.

Proposed: commands that remove more than the user named ask, unless
`-y/--yes` is given or stdin is not a terminal (CI never blocks). Applies
to `uninstall --all` and `venv remove`. Breaking: an interactive
`uninstall --all` now asks.

### 10. Output streams, verbosity and colour

Today results go to stdout (110 `println!`), progress and the venv banner
to stderr (12 `eprintln!`), but not consistently: progress lines inside
`install` go to stdout.

Proposed rule: stdout carries the result of the command and nothing else;
progress, banners, hints and warnings go to stderr. `-q` silences stderr
except errors; `-v` adds detail (VI Server traffic, per-file work).
`--color auto|always|never` and `NO_COLOR` once there is colour. Breaking:
text output, which is not promised.

### 11. `--json`

Proposed: a global `--json`. For `list`, `search`, `targets`, `venv
status`, `config list` and `install --dry-run`, one JSON document on
stdout. For long operations (`install`, `uninstall`, `relink`), one event
per line, as the roadmap's event layer defines. Every object carries
`"schema": 1`. Comes with step 5 of the refactor; the query commands can
have it earlier.

### 12. Exit codes

Today every error is exit 1 through `anyhow`, and clap's usage errors are 2.

Proposed:

| Code | Meaning |
|---|---|
| 0 | done |
| 1 | failed |
| 2 | usage error (clap) |
| 3 | partly done: `uninstall --all`, `relink --all`, `run-hooks --all` with some failures; `install` with unresolved dependencies skipped |
| 4 | the target is busy (another lvpm holds its lock, roadmap *Concurrent callers*, with `--no-wait`) |

`lvpm check` (planned) exits 1 when it finds a problem.

### 13. Smaller items

- `about` still says "proof of concept".
- `search` stops at 40 hits with no way to see more: add `--limit N`
  (`0` for all).
- `cache add` is a group with one member; see *Missing*.
- `lvpm` with no arguments prints help and exits 2. If the TUI ever becomes
  the no-argument default (roadmap), that is a breaking change to decide
  before 1.0, not after.

## Missing

| Command | What | Where it is planned |
|---|---|---|
| `add <pkg>[@constraint]` / `remove <pkg>` | edit `lvpm.toml` and install or uninstall in one step | roadmap, manifest |
| `update [<pkg>]` | re-resolve and rewrite `lvpm.lock` | roadmap, lockfile |
| `outdated` | installed versions against the newest the constraints allow | new; needs the lockfile |
| `info <pkg>` | versions, dependencies, source, license, description before installing | new; the TUI's Browse details need the same |
| `check [DIR]` / `tree [DIR\|VI]` | dependency check and graph | roadmap, dependency analysis |
| `venv rebind` | wipe and bind to another LabVIEW | roadmap, venvs |
| `cache list \| remove \| prune` | complete the cache group | README, known gaps |
| `config list` / `unset` | see item 8 | new |
| `completions <shell>` | shell completion scripts, from `clap_complete` | new |
| `build` | run build specifications | roadmap, building |
| `tui` | terminal UI | roadmap, terminal UI |

## Open decisions

1. **What `install <pkg>` does inside a project once `add` exists.** npm's
   answer: `install <pkg>` adds to the manifest. cargo's: only `add` edits
   the manifest, `install` is for tools. Today `install <pkg>` in a venv
   installs without touching `lvpm.toml`, so the venv holds something the
   manifest does not declare and the lockfile could not reproduce.
   Recommendation: inside a project, `install <pkg>` refuses and points to
   `add`; outside a project it installs as today.
2. **Names for `add` / `remove`.** `remove` next to `uninstall` is
   confusing. Alternative: `add` / `drop`, or keep `uninstall` for both and
   have it edit the manifest inside a project.
3. **`start` and `launch`** (item 5): one command, or two with a clear
   split.
4. **Short flags.** Today only `-y`. Candidates: `-n` for `--dry-run`, `-q`,
   `-v`. Few, chosen once.

## Order

1. Refactor steps 1 and 2 ([refactor-main.md](refactor-main.md)): no
   interface change.
2. Items 1 to 9 and 13: names, flags, aliases. One pull request per item or
   per pair; the README's command table changes with each.
3. Items 10 to 12 with the event layer (refactor steps 4 and 5).
4. Missing commands, lockfile first, after decision 1.
5. Release the frozen interface as 1.0, or as 0.x with this file as the
   promise.
