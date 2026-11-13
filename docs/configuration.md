# Configuration

## Settings

Settings are named the way npm names `npm_config_<key>`: the environment
variable is `LVPM_CONFIG_<KEY>` (or lowercase `lvpm_config_<key>`), and an
empty value counts as unset. Precedence, highest first:

1. Environment variable `LVPM_CONFIG_<KEY>`.
2. User config file at `%LOCALAPPDATA%\lvpm\config.toml` (Linux:
  `$XDG_CONFIG_HOME/lvpm/config.toml`, else `~/.config/lvpm/config.toml`),
  written by `lvpm config set`.
3. Built-in default.

| Variable | Default | What it sets |
| --- | --- | --- |
| `LVPM_CONFIG_CACHE` | `%LOCALAPPDATA%\lvpm\cache` (Linux: `$XDG_CACHE_HOME/lvpm/cache`, else `~/.cache/lvpm/cache`) | Where the cache DB, feed bodies, and imported package archives live |
| `LVPM_CONFIG_SOURCES_LOCAL` | Empty | Local `.vip` directories; separate paths with `;` on Windows or `:` on Linux |
| `LVPM_CONFIG_VENV_DIR` | `.lv-venv` | Project-relative venv directory (or an absolute path) |

```powershell
lvpm config set sources.local D:\LabVIEW\Packages
lvpm config get sources.local
lvpm config set sources.local                 # clear configured local sources
lvpm config set venv.dir .venv
lvpm config get venv.dir
# Or set LVPM_CONFIG_SOURCES_LOCAL for this shell; it overrides config.toml.
$env:LVPM_CONFIG_SOURCES_LOCAL = 'D:\LabVIEW\Packages;E:\MorePackages'
# Relative venv.dir values resolve from each project root; absolute paths are used as-is.
$env:LVPM_CONFIG_VENV_DIR = '.venv'
lvpm search caraya
```

In CI, set `LVPM_CONFIG_CACHE` on the job to keep the cache on a persistent
volume between runs.

## Logs

lvpm keeps one continuous log per venv in `.lv-venv/logs/lvpm.log`. Each command
appends a header line (the command and its directory), everything it printed,
detail it does not print, and how it ended (`---- done` or `---- failed: …`).
Each line carries a UTC timestamp, the process id and a level:

| Level   | What goes there                                                                        |
| ------- | -------------------------------------------------------------------------------------- |
| `error` | why a command failed                                                                   |
| `warn`  | failed hooks, failed uninstalls, files linkfix skipped, download retries               |
| `info`  | everything the command printed                                                         |
| `debug` | file only: every file linkfix changed, every download (URL, size, time) or cache hit   |
| `trace` | not used yet                                                                           |

`--log-level <LEVEL>` (or `LVPM_LOG`) sets the lowest level written; the default is
`debug`, and `off` writes nothing. The console output is the same at every level.
At 10 MB the log moves to `lvpm.log.1`, replacing the previous one. Outside a
venv, and for `lvpm venv remove`, the log is the user's own:
`%LOCALAPPDATA%\lvpm\logs\lvpm.log` on Windows.
