# Build-slice scenarios: the pilots

`s0-pilot-loop.feature` runs the loop every build slice is judged by
(build → publish → install into project B's venv → mass compile) on the two
pilot libraries named in [docs/vision/build-slices.md](../../../docs/vision/build-slices.md#pilot-libraries).

Project B is the consumer project: it depends on both pilots and installs
them from a folder feed of the archives.

## The pilot archives

The `.vip` files are committed in [tests/fixtures/pilots](../../fixtures/pilots),
and [SHA256SUMS](../../fixtures/pilots/SHA256SUMS) holds each one's SHA-256. Every
scenario that reads them checks them against it first, so a changed archive fails the
run instead of testing something else. They are committed rather than
downloaded because the harness keeps `lvpm` off the network, and 2.4 MB is
small. All four are BSD-licensed (HSE Logger: BSD; the LabVIEW Open Source
Project packages: 0BSD), and each archive carries its own `license` file.

| Archive | Why it is here |
|---|---|
| `hse_lib_hse_logger-2.2.2.114.vip` | pilot A |
| `labview_open_source_lib_serializer-1.2.1.16.vip` | pilot B |
| `labview_open_source_lib_data_manipulation-1.6.0.16.vip` | pilot B's one dependency |
| `labview_open_source_project_lib_labview_open_source_palette-1.0.2.7.vip` | Data Manipulation's dependency |

Obtained 2026-10-26 from the public NI index (`download.ni.com`, the source
vipm.io lists) with `lvpm install <name> --prefix <scratch>`, which leaves
each archive in lvpm's cache under its SHA-256. To move a pilot to a newer
version, do the same, copy the archive here, regenerate `SHA256SUMS` with
`sha256sum *.vip`, and update the version in the feature file.

## The pilot sources

S1, S5 and S9 build and check the pilots' sources. They are pinned by commit,
not committed here; the slice that first needs them decides how it fetches
them.

| Pilot | Repository | Tag | Commit |
|---|---|---|---|
| A: HSE Logger 2.2.2.114 | https://code.hampel-soft.com/open-source/hse-logger | `v2.2.2` | `f7cc16a391faa1e982b54ae22d2f43c9df1ef581` |
| B: OpenSerializer 1.2.1.16 | https://github.com/LabVIEW-Open-Source/Serializer | `1.2.1` | `723d7a672b5f0eafa9f98ea63629ba5855d50227` |

## Running them

```console
LVPM_TEST_LABVIEW=1 cargo test --test acceptance
```

The `@labview` scenario is also `@interactive-labview`, so headless CI skips it. It needs:

- LabVIEW 2026, with none of the pilots installed in it (`vi.lib`, `user.lib`, `instr.lib`). The real
  installation wins over a venv, so a pilot there would be tested in place of
  the venv's copy. The scenario fails naming the library it found.
- LabVIEWCLI (`LabVIEWCLI.exe` on `PATH`, or in
  `C:\Program Files (x86)\National Instruments\Shared\LabVIEW CLI`).

It installs without relinking, starts a LabVIEW on the venv, waits for its VI
Server port, and only then runs `LabVIEWCLI -OperationName MassCompile` on
the venv's packages, so LabVIEWCLI never starts a LabVIEW of its own. The
venv is a scratch copy, so the compile works in place. A failed compile names
the VIs the log calls bad. The build and publish scenarios are tagged
`@pending-s1` and `@pending-s2` until those slices land.
