# lvpm vision

**One package manager for LabVIEW: code a library, build it, publish it
anywhere, use it in the next project — safely, per project, and without a
single company owning the feed.**

This folder is this fork's direction. The upstream author's plan stays in
[../roadmap.md](../roadmap.md). The picture is
[lvpm-vision.html](lvpm-vision.html) (open it in a browser; its source is
[lvpm-vision.archify.json](lvpm-vision.archify.json)).

## The loop

| Step | Command | Where it is |
|---|---|---|
| 1. Start a library or app | `lvpm init` | gap |
| 2. Add dependencies | `lvpm add @org/libA@^1.4` | planned |
| 3. Develop in an isolated env | `lvpm sync` → `lvpm launch` | **works** (`install` + venv + `launch`) |
| 4. Build packages | `lvpm build` → `.lvpkg` and/or `.vip` | proposed |
| 5. Publish | tag → CI signs and publishes | proposed |
| 6. Build the application | `lvpm build app` + SBOM | gap |

## Principles

1. **Per project, not per machine.** Every project has its own `.lv-venv`
   and its own LabVIEW process; two projects can use different versions of
   the same package at the same time.
2. **Registry ≠ storage.** Names and hashes live in indexes anyone can host
   or fork; files live wherever the author releases them.
3. **Verify, don't trust.** SHA-256 pinned in the lock, signatures and
   provenance checked against the consumer's policy, no install-time code
   unless allowed by name.
4. **Meet people where they are.** Read VIPM and NI indexes, write `.vip`
   for VIPM users, import `.vipc` projects, check NIPM drivers.
5. **Standard infrastructure.** OCI registries, git, GitHub Releases,
   Sigstore — nothing lvpm-specific to run.

## Milestones

Each one is usable on its own. Ticks mean done.

**M0 — consume (done)**
- [x] Install from VIPM/NI indexes and folders, hooks off by default
- [x] Per-project venvs, `lvpm launch`, relink, refresh, headless
- [x] NIPM dependencies checked

**M1 — reproducible projects**
- [ ] `lvpm.lock` with SHA-256, `lock --check`, `sync` (also removes extras)
- [ ] `init` (incl. `--from project.vipc`), `add`, `remove`, `info`, upgrade
- [ ] `--json`, documented exit codes, `--yes`, `--labview-bitness`

**M2 — build**
- [ ] `lvpm check` / `tree`: linker scan without LabVIEW
- [ ] `lvpm build`: `source` kind, `.lvpkg` format, SBOM
- [ ] `.vip` output, verified by a round-trip install in VIPM
- [ ] `ppl` kind and application builds through `.lvproj` specs

**M3 — publish and trust**
- [ ] GitHub Releases as a direct source, with attestation checks
- [ ] OCI feeds with `@org` scopes
- [ ] CI publishing: OIDC, Sigstore signatures, SLSA provenance, consumer policy

**M4 — community**
- [ ] A federated community index (git, PR-registered, bot-verified)
- [ ] `lvpm convert` for mirroring VIPM packages into internal feeds

## Details

- [distribution-design.md](distribution-design.md) — package kinds and
  formats, build, sources, scopes, security, legacy bridge.
- [vipm-cli-comparison.md](vipm-cli-comparison.md) — VIPM's CLI command by
  command against lvpm (checked 2026-10-24).
