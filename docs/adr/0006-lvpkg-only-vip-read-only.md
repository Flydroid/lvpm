# 6. lvpm writes `.lvpkg` only; `.vip` is read, never written

Date: 2026-10-29

## Status

Accepted. Supersedes [ADR 2](0002-vip-first-lvpkg-later.md). Amends
[ADR 1](0001-manifest-project-and-distribution.md) decisions 2 and 4.

## Context

ADR 2 had `lvpm build` write `.vip` first, so that VIPM users could install
what lvpm builds. S1 built the writer, and lvpm installs its output. S1b
tested whether VIPM installs it too. It does not.

What S1b found, on VIPM 26.3.0 with LabVIEW 2026 64-bit (details in the
[S1b spec](https://github.com/ThatsTheEnd/lvpm/blob/main/docs/specs/lvpm-vip-roundtrip-s1b.md#outcome)):

- **VIPM checks the `ID` key in the `spec`.** It is a 32-digit hex value
  that VIPM's own package builder writes. With the `ID` missing, empty,
  random, or copied from another build, VIPM refuses the package ("file
  format is not valid"). A package built by VIPM's own builder installs.
- **lvpm cannot write that `ID`.** How it is computed is not published. The
  obvious hashes (MD5, SHA-1 and SHA-256 over the spec, the entries or the
  per-file hashes) did not reproduce any of five known IDs. The only way
  left is to study VIPM's code, which [PROVENANCE.md](https://github.com/ThatsTheEnd/lvpm/blob/main/PROVENANCE.md)
  rules out.
- **A passing install can be fake.** VIPM installs its own cached copy when
  the name and version are already known to it, whatever the file holds. The
  first probe run (2026-10-28) reused the published version of pilot A, so
  its 14 "accepted" results only showed that VIPM's cache works. A test of
  VIPM acceptance must use a version VIPM has never seen and compare the
  installed files with the package's own.
- **Building through VIPM is not an option.** `vipm build` builds from a
  `.vipb`, `.lvproj` or `vipm.toml`, but only in public repositories with
  the Community edition, or with VIPM Pro. Few lvpm users have Pro, and
  the Community edition is limited to non-commercial or open-source work.

A `.vip` that VIPM refuses is worse than no `.vip`. It has the extension
every LabVIEW developer knows and VIPM's file icon, and it fails in VIPM
with an error that says the file is broken.

## Decision

1. **`lvpm build` writes `.lvpkg` and nothing else.** S7a replaces S1's
   `.vip` writer. `formats` and `[distribution.vip]` go away, and with them
   the name map for scoped dependencies, because `.lvpkg` has scoped names.
2. **lvpm keeps reading and installing `.vip` and `.ogp`.** The VIPM and NI
   indexes, folder feeds, the cache, GitHub release files and `lvpm init
   --from project.vipc` all keep working. `lvpm convert x.vip` (not sliced
   yet) turns an existing VIPM package into a `.lvpkg`.
3. **lvpm never calls VIPM to build.** No `vipm build` and no `.vipb`
   generation.
4. **Kept from ADR 2:** the output is deterministic (decision 2), and
   `lvpm build` writes no install hooks (decision 6).
5. **Revisit only if JKI publishes how the `ID` is computed.** Then a `.vip`
   writer could come back as an extra format. Until then nobody probes VIPM
   for it again.

## Consequences

- A library built by lvpm cannot be installed with VIPM. VIPM users install
  the library's own VIPM release, or use lvpm. Principle 4 in the vision
  changes from "write `.vip` for VIPM users" to "read `.vip`".
- S1b closes without its round trips. Its milestone ("`.vip` output, verified
  by a round-trip install in VIPM") is dropped.
- S7a has landed. It depended only on S1. The SBOM inside a `.lvpkg` moves to
  S9, which now adds it.
- `lvpm publish` (S2) copies the `.lvpkg`. GitHub release lookup (S4) takes
  `*.lvpkg` first and `*.vip` as a fallback, so existing VIPM releases stay
  installable. S4's CI template can ship now that S7a has merged, so it never
  uploads an lvpm-built `.vip`.
- `lvpm build` no longer writes a `.vip`. No lvpm-built `.vip` was ever
  released, so nothing needed migrating.
- The `.vip` writer (`spec::write`) and the probe runner
  (`tests/vipm_probe.rs`) were deleted by S7a. Git history keeps them; their
  "accepted" verdict was not proof for a reused version (see above).
