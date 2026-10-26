# 2. `.vip` first, `.lvpkg` later

Date: 2026-10-26

## Status

Accepted

## Context

`lvpm build` (S1) has to write one package format first. There are two:

- **`.vip`**: a zip holding an OpenG `spec` file and `File Group N/` folders.
  VIPM installs it, and lvpm already reads and installs it (`spec.rs`,
  `install.rs`). It has no scoped names, 4-part versions, no hashes or
  signature inside, and optional install hooks (`[Script VIs]`).
- **`.lvpkg`**: lvpm's own format, designed in
  [distribution-design.md](../vision/distribution-design.md) and not built
  yet. A deterministic zip with `package.toml`, `RECORD` (a SHA-256 per file)
  and a CycloneDX SBOM. Only lvpm will read it.

Neither format is needed for signatures or OCI feeds. A GitHub artifact
attestation signs any file's digest, and an OCI registry can hold any file as
an artifact, with referrers attached to its digest. What `.lvpkg` adds is
`RECORD` (installed files can be checked later), the SBOM inside the package,
scoped names with SemVer, and no install hooks by design.

Whether VIPM accepts a `.vip` lvpm wrote is the biggest unknown in the plan.
VIPM's current `spec` format (`File Format = "vip"`, `Format Version =
"2017"`) is not published; lvpm knows it only from observed files.

## Decision

1. **S1 writes `.vip`. `.lvpkg` follows in S7a.**
   - Every LabVIEW shop already installs `.vip`, so a library lvpm builds is
     usable by VIPM users from the first release.
   - Writing a `.vip` is the inverse of `spec::parse`, so S1 stays small.
   - S1b settles the VIPM unknown early, before S2, S4 and S7a build on it.
2. **The output is deterministic.** Building the same source twice with the
   same lvpm gives the same bytes: sorted entries, one fixed timestamp, zeroed
   attributes, a fixed `spec` key order, VI files copied and never resaved.
   Matching VIPM's own output byte for byte is not a goal, since VIPM's output
   isn't reproducible itself.
3. **What VIPM requires is learned by black-box probing** (S1b): published
   spec files, VIPM-built reference archives, and `.vip` variants installed in
   VIPM. Nothing is decompiled or disassembled, as
   [PROVENANCE.md](../../PROVENANCE.md) requires.
4. **Until S7a, `.vip` gaps are covered outside the file.**
   - Scoped dependency names use `[distribution.vip.dependency-names]`.
   - The signature is a detached attestation (S6).
   - The SBOM is a separate file (S9).
   - The whole-file SHA-256 in `lvpm.lock` protects the download.
5. **`.vip` stays a first-class format for good.** Every lvpm feature works
   on a `.vip`-only package: locks, attestation policy, scoped names through the name
   map, every feed. The one exception is per-file `RECORD` checks, which only
   `.lvpkg` has. A library author never has to pick `.lvpkg` for security
   reasons.
6. **`lvpm build` never writes install hooks** (`[Script VIs]`) for now.
   Installing a package lvpm built runs no code, in VIPM or in lvpm. Pilot A's
   own `PostInstall.vi` only opens HSE's documentation page (read from the
   VI's strings, not run), so nothing is lost. An opt-in can be added later if
   a real library needs one; that change is additive. How lvpm treats hooks in
   packages it installs is [ADR 3](0003-install-hooks-off-by-default.md).

## Consequences

- Moving to `.lvpkg` is additive: `formats = ["lvpkg", "vip"]` writes both,
  and GitHub release lookup prefers `*.lvpkg` and falls back to `*.vip`, so
  existing releases keep working.
- If VIPM rejects lvpm's `.vip` in S1b, `.vip` stays an lvpm-only format and
  S7a moves earlier. S2, S4 and S7a are re-planned before any of them merges.
