# 1. A project and its venv

## Make a folder

```console
$ mkdir C:\Git\HelloLvpm
$ cd C:\Git\HelloLvpm
```

## Write lvpm.toml

`lvpm.toml` says what the project is and what it depends on. There is no
`lvpm init` yet, so write it by hand. Create `lvpm.toml` with:

```toml
[project]
name = "hello_lvpm"
labview = ">=2026"   # the oldest LabVIEW the project is meant for

[dependencies]
```

- `name` is the project's name, and later the name of its package.
- `labview` is a minimum, not a pin. lvpm needs it to lock, install and
  add, and refuses a bare year: write `">=2026"`, not `"2026"`.
- `[dependencies]` is empty for now; `lvpm add` fills it in the next step.

Any `lvpm` command run in this folder, or below it, now works on this
project.

## Create the venv

A venv keeps the project's packages to itself instead of putting them into
LabVIEW's own `vi.lib`, so another project on the machine can use other
versions.

```console
$ lvpm venv create --labview-version 2026
created C:\Git\HelloLvpm\.lv-venv
  bound to       LabVIEW 2026 (64-bit)  v26.3
  VI Server port 3735
```

The venv is `.lv-venv\`, beside `lvpm.toml`, bound to LabVIEW 2026. Without
`--labview-version` lvpm takes the manifest's `labview`. The venv ignores
itself in git; you commit `lvpm.toml`, not the venv.

Check it:

```console
$ lvpm venv status
```

There is no activation step. Like cargo or npm, every `lvpm` command in this
folder uses this venv and prints which one it resolved to.

Next: [Add a dependency, open LabVIEW](add-and-launch.md).
