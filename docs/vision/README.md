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
| 1. Start a library or app | `lvpm init` | planned (M1) |
| 2. Add dependencies | `lvpm add libA@">=1.4"` | **works** (`name@constraint`; `@org` scopes planned, M3; `^` not yet) |
| 3. Develop in an isolated env | `lvpm install` → `lvpm launch` | **works** (venv + overlay, `lvpm.lock`, exact `sync`) |
| 4. Build packages | `lvpm build` → `.lvpkg` and/or `.vip` | proposed (M2) |
| 5. Publish | tag → CI signs and publishes | proposed (M3) |
| 6. Build the application | `lvpm build` with a `.lvproj` build spec, + SBOM | proposed (M2) |

## Principles

1. **Per project, not per machine.** Every project has its own `.lv-venv`
   and its own LabVIEW process; two projects can use different versions of
   the same package at the same time. (Today a global install of the same
   package still shadows the venv copy, without a warning.)
2. **Registry ≠ storage.** Names and hashes live in indexes anyone can host
   or fork; files live wherever the author releases them.
3. **Verify, don't trust.** SHA-256 pinned in the lock, signatures and
   provenance checked against the consumer's policy, no install-time code
   unless allowed by name. (Target: today hooks still run by default on a
   developer machine.)
4. **Meet people where they are.** Read VIPM and NI indexes, write `.vip`
   for VIPM users, import `.vipc` projects, check NIPM drivers.
5. **Standard infrastructure.** OCI registries, git, GitHub Releases,
   Sigstore — nothing lvpm-specific to run.

## Milestones

Each one is usable on its own. Ticks mean done.

**M0 — consume (done)**
- [x] Install from VIPM/NI indexes and folders; hooks run on a developer machine, never run in a venv, and are skipped headless unless `--hooks` is given
- [x] Per-project venvs, `lvpm launch`, relink, refresh, headless
- [x] NIPM dependencies checked

**M1 — reproducible projects**
- [x] `lvpm.lock` with SHA-256, `lock --check`, `sync` (also removes extras)
- [x] `add`, `remove` (`--dev`)
- [ ] `init` (incl. `--from project.vipc`), `info`, `update`
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
- [build-slices.md](build-slices.md) — the vision cut into slices that the
  dev-team pipeline ships one PR at a time.
- [vipm-cli-comparison.md](vipm-cli-comparison.md) — VIPM's CLI command by
  command against lvpm (checked 2026-10-24).
