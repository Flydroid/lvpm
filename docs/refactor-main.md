# Splitting `main.rs`

`src/main.rs` is 1,856 lines, the largest file in the crate, and holds about
eight unrelated concerns: the clap definitions, destination resolution, the
dependency resolver, install/uninstall/relink orchestration, hook running,
venv commands, settings, and the VI Server debug primitives. It prints from
130 places. Every other module has unit tests; `main.rs` has none, mostly
because its testable logic is inlined into I/O-heavy command functions.

This is the first step of the [roadmap](roadmap.md)'s Terminal UI work, and
the CLI changes in [cli.md](cli.md) land in the files it creates. Line
numbers below are as of `main` at `39a5aaf`.

## Why now

- **The resolver is buried in `cmd_install`.** The depth-first walk
  ([main.rs:820-881](../src/main.rs#L820-L881)) sits inside a 412-line
  function. The lockfile, `lvpm update` and `lvpm tree` all need it as a
  callable function, and extracting it is what makes it testable.
- **The roadmap adds commands:** `init`, `add`, `remove`, `update`, `check`,
  `tree`, `build`, `venv rebind`, `tui`. Each would otherwise land in
  `main.rs`.
- **Duplication:**
  - "Start the venv's LabVIEW, relink, shut it down unless `--keep-open`" is
    written twice, near-verbatim
    ([main.rs:1088-1097](../src/main.rs#L1088-L1097),
    [main.rs:1438-1447](../src/main.rs#L1438-L1447)).
  - "`roots_for`, then bail with *needs a real LabVIEW target*" appears 6
    times (`start`, `refresh`, `run-hooks`, `relink`, `app-probe`,
    `vi_server_for`).
  - `read_spec` ([main.rs:1843](../src/main.rs#L1843)) duplicates the spec
    lookup in `index::entry_from_vip` ([index.rs:182](../src/index.rs#L182)).
- **Argument lists:** `cmd_install` takes 10 parameters, `cmd_vi_run` 9, and
  `main()` destructures every field by hand. `#[derive(Args)]` structs (as
  `RelinkArgs` already is) turn each into one `&InstallArgs` and each dispatch
  arm into one line.

## Target layout

Two sides, kept apart so the TUI and `--json` can reuse the second without
the first:

- **`cmd/`, the CLI side.** Takes parsed arguments, builds the `Context`,
  prints. One file per group of commands.
- **Library modules** (`install.rs`, `relink.rs`, `venv.rs`, … as today,
  plus `context.rs`, `resolve.rs`, `hooks.rs`). Never parse arguments. After
  phase 3 they never print either.

| File | Contents | ~Lines |
|---|---|---|
| `main.rs` | `mod` declarations, `main()`, dispatch, exit code | 60 |
| `cli.rs` | `Cli`, `Cmd`, `VenvCmd`, `CacheCmd`, `ConfigCmd`, `RelinkArgs`, new `InstallArgs` / `ViRunArgs` | 260 |
| `context.rs` | `Destination`, `destination`, `roots_for`, `headless`, `headless_roots`, `manifest_for`, `load_index`, `work_dir` (from #9), new `require_target()` | 180 |
| `resolve.rs` | `Req`, `resolve(...) -> Vec<Entry>`, **tests** | 130 |
| `hooks.rs` | `hook_action_info`, `run_hook_vi`, `run_post_install_hooks`, `hook_names`, `HOOK_TIMEOUT_SECS` | 140 |
| `cmd/install.rs` | `cmd_install` minus the resolver; download + MD5 check as a helper | 300 |
| `cmd/uninstall.rs` | `cmd_uninstall`, `uninstall_one` | 130 |
| `cmd/relink.rs` | `cmd_relink`, `run_relink`, shared venv-or-global relink helper | 140 |
| `cmd/maint.rs` | `cmd_run_hooks`, `cmd_refresh` | 80 |
| `cmd/venv.rs` | `cmd_venv` | 60 |
| `cmd/launch.rs` | `cmd_launch`, `cmd_start` (see [cli.md](cli.md) on merging them) | 60 |
| `cmd/settings.rs` | `config get`/`set`, `cache add` (both inline in `main()` today) | 50 |
| `cmd/query.rs` | `targets`, `search`, `list` | 70 |
| `cmd/debug.rs` | `app-probe`, `vi-probe`, `vi-save`, `vi-run`, `vi_server_for`, `parse_set`, `scalar`, `element_code`, `poll_progress`, **`parse_set` tests** | 300 |

`read_spec` moves into the top-level `install.rs` next to `open_archive`, and
`index::entry_from_vip` calls it instead of repeating it.

`refresh_palettes_and_menus` moves into the top-level `refresh.rs`: install
and uninstall call it as much as `lvpm refresh` does.

`cmd/` is named to avoid a clash with the existing top-level `install.rs` and
`relink.rs`, which stay as they are: they are the library side.

## Defects to fix on the way

- `run_relink`'s doc comment ends up above `cmd_run_hooks`
  ([main.rs:1148](../src/main.rs#L1148)), so rustdoc merges the two. The
  real `run_relink` at [main.rs:1329](../src/main.rs#L1329) has its own.
- Four `println!` strings contain a literal line break where `\n` was meant
  ([main.rs:898](../src/main.rs#L898), [1111](../src/main.rs#L1111),
  [1119](../src/main.rs#L1119), [1218](../src/main.rs#L1218)). Output is the
  same; it reads like an editing accident.
- Dead code in `poll_progress`: `sum` is computed and discarded with
  `let _ = sum;` ([main.rs:1774](../src/main.rs#L1774),
  [1782](../src/main.rs#L1782)).

## Order of work

1. **Pure moves.** No behaviour or output change, one commit per new module.
   `cargo build`, `cargo clippy` and `cargo test` after each, on Windows and
   Linux; the e2e suite once at the end. One pull request, reviewed as a
   move (`git diff --color-moved`).
2. **Cleanups**, as separate commits:
   - extract `resolve()` and add tests for it (exact pins, `>=` floors, `*`,
     installed versions that satisfy a dependency, downgrades);
   - switch `Install` and `ViRun` to `Args` structs;
   - add `require_target()` and the shared relink helper to remove the
     duplication;
   - the three defect fixes above.
3. **CLI changes** from [cli.md](cli.md), now that each touches a small file:
   `cli.rs` for names and flags, `context.rs` for making every command
   resolve its destination the same way.
4. **Plan, then execute.** `cmd_install` becomes resolve → `Plan` → execute;
   `--dry-run` prints the `Plan`. The execute half moves into the library
   side.
5. **Event layer** (roadmap, *Event layer and JSON interface*). Library
   functions take a `Sink` instead of printing; `cmd/` supplies the text
   sink, `--json` the JSON one. Exit codes are settled here, with
   `ErrorReport`.

Steps 1 and 2 change no behaviour. Steps 3 to 5 change the interface and
follow [cli.md](cli.md).

## Coordination

Every open branch that touches `main.rs` conflicts with step 1, and a move
cannot be rebased onto, only replayed by hand. So: land what is ready, then
freeze `main.rs` for the length of one pull request.

Branches that change `main.rs`, against `main` at `39a5aaf`:

| Branch | PR | `main.rs` | Ahead / behind | Before or after step 1 |
|---|---|---|---|---|
| `feat/venv-isolation` | #17 | +11 −4 | 3 / 2 | before |
| `docs/project-manifest` | #16 | +1 −1 | 1 / 2 | before |
| `claude/lvpm-linux-build-ke4hoh` (+ `claude/windows-container-e2e` on top) | #9 | +32 −6 | 27 / 6 | before: brings `work_dir` and the Linux CI that checks step 1 |
| `feature/linkfix` | none | +310 −101 | 6 / 17 | decide first: merge before step 1, or replay onto `cmd/relink.rs` after |
| `feature/pack` | none | +49 | 3 / 13 | after: mostly a new `pack.rs`, the `main.rs` part is a new command |
| `feature/index-cache-revalidate` | none | +2 −1 | 1 / 15 | check whether `main` already has it; close |
| `feature/embed-relink-vi` (local only) | none | +149 −57 | 3 / 38 | likely superseded by the embedded relink VI on `main`; close |

Sequence:

1. Merge #16, #17, #9 (with `windows-container-e2e`) and #15 (this plan and
   the roadmap).
2. Decide `feature/linkfix`.
3. Announce the freeze, open the step 1 pull request, merge it the same day.
4. Rebase or replay the remaining branches; continue with steps 2 to 5 as
   ordinary pull requests.
