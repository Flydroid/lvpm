# 2. Add a dependency, open LabVIEW

## Find a package

```console
$ lvpm search error
$ lvpm info oglib_error
```

`search` looks through the package indexes; `info` shows one package's
description, requirements and versions.

## Add it

```console
$ lvpm add oglib_error
```

That one command does three things:

1. Adds `oglib_error = ">=<the version it locked>"` under `[dependencies]`
   in `lvpm.toml`.
2. Resolves every package, direct or transitive, and writes the exact
   versions, SHA-256 hashes and sources to `lvpm.lock`.
3. Syncs: installs exactly what the lock lists into `.lv-venv`, resaves the VIs in
   the venv's LabVIEW and relinks them. lvpm starts that LabVIEW itself,
   so this takes a minute or so per package.

Look at `lvpm.toml` and `lvpm.lock` now. Commit both: a teammate who runs `lvpm venv create` and `lvpm sync` in a
fresh clone, or a CI job that runs `lvpm sync`, gets the same packages, byte
for byte.

```console
$ lvpm list
```

shows what is installed in the venv.

## Open LabVIEW on the venv

```console
$ lvpm launch
```

starts a LabVIEW that sees the venv: the installed packages appear in the
palettes and resolve as `<vilib>/...`, exactly as if they were installed in
LabVIEW itself, while `C:\Program Files` stays untouched. Give it a project
file to open it straight away: `lvpm launch HelloLvpm.lvproj`. A LabVIEW
you already had open is left alone.

Save a project (`HelloLvpm.lvproj`) and a VI that uses one of the package's
VIs into the folder. The VI links to the package by a portable path, so it works on
any machine with the same LabVIEW version that syncs the same lock.

After a later `lvpm add`, restart with `lvpm launch`: LabVIEW reads the venv
only when it starts.

Next: [Package and publish a library](build-and-publish.md).
