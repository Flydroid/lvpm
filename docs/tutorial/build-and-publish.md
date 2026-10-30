# 3. Package and publish a library

Now turn code of your own into a package that other projects install with
`lvpm add`.

## A library to package

`lvpm build` packages one LabVIEW library (`.lvlib`) and everything it lists.
In the LabVIEW you opened with `lvpm launch`, create a library
`Source\Hello\hello.lvlib` in the project folder and save a VI or two into
it, in the same folder.

## Describe the package

Give the project a version and add a `[distribution]` table to `lvpm.toml`:

```toml
[project]
name = "hello_lvpm"
version = "0.1.0"    # 3 or 4 numeric parts
labview = ">=2026"

[distribution]
top-level-library = "Source/Hello/hello.lvlib"   # relative to lvpm.toml, forward slashes
vendor       = "Hello Inc"
display-name = "Hello"

[dependencies]
oglib_error = ">=6.0.1"   # what lvpm add wrote; yours may differ
```

The package installs below `vi.lib/<vendor>/<display-name>`, here
`vi.lib/Hello Inc/Hello`; `install-dir` sets another folder. The
`[dependencies]` go into the package as they are, so whoever installs it
gets `oglib_error` too.

## Build

```console
$ lvpm build
```

writes `dist/hello_lvpm-0.1.0.lvpkg`. It needs no LabVIEW, and the same
sources always give the same bytes. A build that fails names the key or file
at fault.

A `.lvpkg` is lvpm's own format; VIPM does not read it
([ADR 6](../adr/0006-lvpkg-only-vip-read-only.md)).

## Publish to a folder feed

```console
$ mkdir D:\feeds\packages
$ lvpm publish --to D:/feeds/packages
published D:\feeds\packages\hello_lvpm-0.1.0.lvpkg
```

A published file is never replaced: to publish a change, raise `version`
and build again.

## Install it from another project

Point lvpm at the feed. Package folders are configured per user, not per
project ([Package sources](../usage.md#package-sources)):

```console
$ lvpm config set sources.local D:\feeds\packages
```

Then, in another project with its own `lvpm.toml` and venv (steps 1 and 2
again, in a new folder):

```console
$ lvpm add hello_lvpm
```

`hello_lvpm` and its `oglib_error` land in that project's venv.

## Where next

- Every command and manifest key: [Commands and configuration](../usage.md).
- Running all of this headless: [Run lvpm in CI](../ci-docker.md).
- Why packages look the way they do: [The OpenG package model](../ogpm-model.md).
