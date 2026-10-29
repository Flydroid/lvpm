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
small. Each archive carries its own `license` file. The LabVIEW Open Source
Project packages are 0BSD. HSE Logger's spec says "BSD License", but its
`license` file is a modified BSD that forbids redistribution "in source
form". A `.vip` holds source VIs, so whether committing this archive is
allowed is an open question for its vendor (see the S1 decisions log). The license
file was read on 2026-10-26.

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

S1, S5 and S9 build and check the pilots' sources. They are pinned by commit
and not committed here. Pilot A's LICENSE forbids redistributing it in source
form, so the harness never fetches or copies it anywhere but a test sandbox.
The `@pilot-source` scenarios read it from a checkout that you make:

```console
git clone https://code.hampel-soft.com/open-source/hse-logger
git -C hse-logger checkout f7cc16a391faa1e982b54ae22d2f43c9df1ef581
LVPM_TEST_PILOT_A=<path to hse-logger> cargo test --test acceptance
```

Pilot B's `@pilot-b-source` scenario works the same way with its own variable:

```console
git clone https://github.com/LabVIEW-Open-Source/Serializer
git -C Serializer checkout 723d7a672b5f0eafa9f98ea63629ba5855d50227
LVPM_TEST_PILOT_B=<path to Serializer> cargo test --test acceptance
```

Each scenario checks that its HEAD is the pinned commit and that the folder
the build copies (`Source` or `source`) has no modified, untracked or ignored
files; changes elsewhere in the checkout are accepted. Without the variable, the harness does not run that pilot's scenarios
and says how many it left out.

| Pilot | Repository | Tag | Commit |
|---|---|---|---|
| A: HSE Logger 2.2.2.114 | https://code.hampel-soft.com/open-source/hse-logger | `v2.2.2` | `f7cc16a391faa1e982b54ae22d2f43c9df1ef581` |
| B: OpenSerializer 1.2.1.16 | https://github.com/LabVIEW-Open-Source/Serializer | `1.2.1` | `723d7a672b5f0eafa9f98ea63629ba5855d50227` |

## The VIPM probe (S1b)

The probe installs lvpm's build of pilot A and its variants through VIPM
into the LabVIEW 2026 installation and records which ones VIPM accepts. It is
an `#[ignore]` test, so plain `cargo test` never runs it. It needs `vipm.exe`
and pilot A's checkout (as above), no LabVIEW running, and none of the pilots
installed:

```console
LVPM_TEST_VIPM=<path to vipm.exe> LVPM_TEST_PILOT_A=<path to hse-logger> cargo test --test vipm_probe -- --ignored --nocapture
```

The findings table and VIPM's output for each variant go to
`target/vipm-probes/<timestamp>/`, which is never committed. If a run is
interrupted, the next one names the packages it left installed.

Its "accepted" proves nothing for a name and version VIPM already has: VIPM
then installs its cached copy. Every variant keeps pilot A's published
version, so a run only tells whether VIPM's cache works. VIPM refuses lvpm's
`.vip` at a new version, and lvpm no longer aims to write one VIPM accepts
([ADR 6](../../../docs/adr/0006-lvpkg-only-vip-read-only.md)). The round-trip
scenarios were removed for that reason.

## Running them

```console
LVPM_TEST_LABVIEW=1 cargo test --test acceptance
```

The `@labview` scenarios are also `@interactive-labview`, so headless CI skips them. They need:

- LabVIEW 2026, with none of the pilots installed in it (`vi.lib`, `user.lib`, `instr.lib`). The real
  installation wins over a venv, so a pilot there would be tested in place of
  the venv's copy. The scenario fails naming the library it found.
- LabVIEWCLI (`LabVIEWCLI.exe` on `PATH`, or in
  `C:\Program Files (x86)\National Instruments\Shared\LabVIEW CLI`).

It installs without relinking, starts a LabVIEW on the venv, waits for its VI
Server port, and only then runs `LabVIEWCLI -OperationName MassCompile` on
the venv's packages, so LabVIEWCLI never starts a LabVIEW of its own. The
venv is a scratch copy, so the compile works in place. A failed compile names
the VIs the log calls bad.

Scenarios that use the installation's own LabVIEW first claim it with a file
in `%TEMP%`, so two test runs never share one. If another run holds the
claim, or something already answers VI Server on the installation's port
(your own LabVIEW, say), they are not run and the harness says why. The
LabVIEW the run started is ended when the run ends, even after a panic.
