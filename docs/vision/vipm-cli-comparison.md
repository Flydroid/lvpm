# VIPM CLI compared with lvpm

Checked on 2026-10-24 against the VIPM
[2026-Q1 command reference](https://docs.vipm.io/2026-Q1/cli/command-reference/),
the [preview command reference](https://docs.vipm.io/preview/cli/command-reference)
and the [vipm.toml guide](https://docs.vipm.io/2026-Q1/vipm-toml/getting-started/).
VIPM's `vipm.toml` and NIPM integration are previews and may change.

VIPM is moving towards the uv model: `vipm.toml`, `vipm.lock` with SHA-256,
dev dependencies, `add` / `remove` / `lock` / `sync`, and NIPM dependencies
in `[nipm.*]`. What VIPM's docs do not show: per-project environments (they
say only that packages are installed "to LabVIEW"), a publish command
(publishing is a web upload to vipm.io), signing or provenance.

Status:

- **have**: in lvpm today;
- **partial**: some of it exists;
- **planned**: in [../roadmap.md](../roadmap.md), [distribution-design.md](distribution-design.md)
  or the [vision milestones](README.md#milestones) (milestone given);
- **open**: found here, not planned yet.

| VIPM | lvpm | Status |
|---|---|---|
| `install <pkg>[@ver]` | `lvpm install` | have |
| `install <file.vip / .ogp>` | `lvpm cache add <file>`, then `install` | have (two steps) |
| `install <file.vipc / .dragon>` | `lvpm init --from <file>` | planned (M1) |
| `install --upgrade` | upgrade | planned (roadmap, M1) |
| `uninstall` | `lvpm uninstall` / `--all` | have |
| `uninstall --allow-version-mismatch` | — | open |
| `list`, `list --installed` | `lvpm list` | have |
| `info <pkg>` (versions, deps, files) | `lvpm info` | planned (M1) |
| `search` | `lvpm search` | have |
| `refresh` (`--no-cache`, `--force`) | `--refresh` flag | have |
| `init` | `lvpm init` | planned (M1) |
| `add` / `remove` (`--dev`, `--feed-url`) | `lvpm add` / `remove` | planned (roadmap, M1) |
| `lock`, `lock --check` (non-zero exit on drift) | `lvpm.lock`, `lock --check` | planned (roadmap, M1) |
| `sync` (make the install match the lock) | `lvpm sync` | planned (M1) |
| `build` of packages (`vipb`) | `lvpm build` | planned (M2) |
| `build` of `ppl`, `exe`, `lvproj` (`depends-on`, version/build number) | `lvpm build` with `.lvproj` specs | planned (roadmap "PPL support", M2) |
| `clean` | — | open |
| `sbom` (for an application / build spec) | SBOM per package and per application build | planned (M2) |
| `install --nipm`, `add --nipm --feed-url` | `[nipm.dependencies]` checked, not installed | partial; installing via `nipkg` is open |
| `labview list` | `lvpm targets` | have |
| `version` | `lvpm --version` | have |
| `about` (paths, config) | — | open |
| `activate` | — | not needed (no licensing) |

Global behaviour:

| VIPM | lvpm | Status |
|---|---|---|
| `--labview-version` | `--labview-version` | have |
| `--labview-bitness` | — | planned (M1) |
| `--json` | — | planned (M1) |
| documented exit codes (27 codes, e.g. 7 dependency conflict, 17 package drift, 24 VI Server unavailable) | — | planned (M1) |
| `--yes`, `VIPM_ASSUME_YES`, `VIPM_NONINTERACTIVE` | `venv remove -y/--yes` only | partial; global `--yes` planned (M1) |
| `--timeout` | on `run-hooks`, `refresh`, `vi-run` | partial |
| `--verbose`, `--show-progress` | — | open |
| `--best-effort`, `--allow-package-drift` | — | open |
| `http_proxy` / `https_proxy` | not verified | open |

What lvpm has that VIPM's docs do not show:

- per-project venvs, each with its own LabVIEW process;
- install hooks skipped in a venv and on headless machines, with `--hooks` to opt in;
- explicit relink control;
- VI Server primitives for diagnosis.
