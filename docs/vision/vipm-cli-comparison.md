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

Status: **have** = in lvpm today, **planned** = already in lvpm's roadmap
or [distribution-design.md](distribution-design.md), **gap** = found here.

| VIPM | lvpm | Status |
|---|---|---|
| `install <pkg>[@ver]` | `lvpm install` | have |
| `install <file.vip / .ogp>` | `lvpm cache add <file>`, then `install` | have (two steps) |
| `install <file.vipc / .dragon>` | — | **gap**: `lvpm init --from <file>` for migration |
| `install --upgrade` | — | **gap** (roadmap has "upgrade" from OGPM) |
| `uninstall`, `--allow-version-mismatch` | `lvpm uninstall` / `--all` | have; mismatch flag is a gap |
| `list`, `list --installed` | `lvpm list` | have |
| `info <pkg>` (versions, deps, files) | — | **gap** |
| `search` | `lvpm search` | have |
| `refresh` (`--no-cache`, `--force`) | `--refresh` flag | have |
| `init` | — | **gap** |
| `add` / `remove` (`--dev`, `--feed-url`) | — | planned |
| `lock`, `lock --check` (exit 2 on drift) | — | planned (lockfile); **gap**: the `--check` CI mode |
| `sync` (make the install match the lock) | `lvpm install` in a project | **gap**: remove packages the lock doesn't list |
| `build` (`[build.*]`: ppl, exe, lvproj, vipb; `depends-on`; version/build number) | — | **gap**: package build is planned; app/PPL builds are not |
| `clean` | — | **gap** (small) |
| `sbom` (for an application / build spec) | — | **gap**: planned per package, not per application (EU CRA) |
| `install --nipm`, `add --nipm --feed-url` | `[nipm.dependencies]` checked | **gap**: optional install via `nipkg`, machine-wide opt-in |
| `labview list` | `lvpm targets` | have |
| `version`, `about` | `lvpm --version` | have; `about` (paths, config) is a small gap |
| `activate` | — | not needed (no licensing) |

Global behaviour:

| VIPM | lvpm | Status |
|---|---|---|
| `--labview-version` | `--labview-version` | have |
| `--labview-bitness` | — | **gap** (roadmap mentions `[venv] bitness`) |
| `--json` | — | **gap**: needed for CI and tooling |
| documented exit codes (27, e.g. 7 dependency conflict, 17 drift, 24 VI Server unavailable) | — | **gap** |
| `--yes`, `VIPM_ASSUME_YES`, `VIPM_NONINTERACTIVE` | — | **gap** |
| `--timeout`, `--verbose`, `--show-progress` | — | **gap** |
| `--best-effort`, `--allow-package-drift` | — | **gap** |
| `http_proxy` / `https_proxy` | — | unverified for lvpm's HTTP client |

What lvpm has that VIPM's docs do not show: per-project venvs with a separate
LabVIEW process each, hooks off by default, headless installs, explicit
relink, and VI Server primitives for diagnosis.
