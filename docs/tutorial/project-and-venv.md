# 1. A project and its venv

## Make a folder

```console
$ mkdir C:\Git\hello_lvpm
$ cd C:\Git\hello_lvpm
```

## Start the project

```console
$ lvpm init --labview-version 2026
wrote C:\Git\hello_lvpm\lvpm.toml
  labview = ">=2026"
initialized a git repository
  add a remote:  git remote add origin <url>
  first push:    git add -A && git commit -m "lvpm init" && git push -u origin HEAD
created C:\Git\hello_lvpm\.lv-venv
  bound to       LabVIEW 2026 (64-bit)  v26.3
next: lvpm sync
```

`lvpm init` writes `lvpm.toml`, the project manifest, which says what the
project is and what it depends on, makes a git repository and creates the
venv. It reads:

```toml
[project]
name = "hello_lvpm"
version = "0.1.0"
labview = ">=2026"   # the oldest LabVIEW the project is meant for

[dependencies]
```

- `name` is the project's name, and later the name of its package.
- `labview` is a minimum, not a pin. lvpm needs it to lock, install and
  add, and refuses a bare year: write `">=2026"`, not `"2026"`.
- `[dependencies]` is empty for now; `lvpm add` fills it in the next step.

Any `lvpm` command run in this folder, or below it, now works on this
project.

## The venv

A venv keeps the project's packages to itself instead of putting them into
LabVIEW's own `vi.lib`, so another project on the machine can use other
versions.

The venv is `.lv-venv\`, beside `lvpm.toml`, bound to LabVIEW 2026. It
ignores itself in git; you commit `lvpm.toml`, not the venv. If init said
`no venv:` instead, fix what it names and run
`lvpm venv create --labview-version 2026`.

Check it:

```console
$ lvpm venv status
```

There is no activation step. Like cargo or npm, every `lvpm` command in this
folder uses this venv and prints which one it resolved to.

Next: [Add a dependency, open LabVIEW](add-and-launch.md).
