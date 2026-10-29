# Distribution design: build, publish, consume

Status: **proposal**. Nothing in this document is implemented unless it says
so. It starts from current practice (uv/PyPI, Cargo, Go modules, OCI
registries, Sigstore), not from the OGPT design in [../ogpm-model.md](../ogpm-model.md).

The workflow it serves:

1. code a library;
2. build it into a package;
3. publish it to a folder, a hosted feed (GitHub, GitLab, Azure) or a public index;
4. use it to develop library B or an application.

## Terms

| Term | Meaning here |
|---|---|
| **kind** | what a package contains: `source` or `ppl` |
| **format** | the container: `.lvpkg` (written from S7a, and read) or `.vip` (read; written until S7a for lvpm only) |
| **feed** | any place packages are resolved from (folder, index, OCI registry, GitHub repo); the existing `[sources]` table and `sources.local` setting configure feeds |
| `lvpm.toml` | the project manifest a developer edits; its `[project]` table names the package and its `[distribution]` table says what to ship |
| `package.toml` | the metadata file `lvpm build` writes inside each `.lvpkg`, generated from `lvpm.toml` |

## 1. Package: two axes, not one

A package has a **kind** (what is inside) and a **format** (the container).
They are chosen independently.

| Kind | Contents | Tag | Python analogue |
|---|---|---|---|
| `source` (default) | VIs saved at the declared minimum LabVIEW version, separate compiled code | `lv2021-any` | pure wheel (`py3-none-any`) |
| `ppl` | PPLs, DLLs, .NET assemblies | `lv2024-x64` | platform wheel |

A LabVIEW source package needs no build step on the consumer's machine, so it is a
pure wheel rather than an sdist.

| Format | Purpose | Carries signature/provenance |
|---|---|---|
| `.lvpkg` | native: deterministic zip, LVAddons layout, `package.toml`, `RECORD` (SHA-256 per file), CycloneDX SBOM | yes (OCI referrers or a Sigstore bundle) |
| `.vip` | read: packages from VIPM and NI indexes, folders and releases; written by `lvpm build` for lvpm only until S7a ([ADR 6](../adr/0006-lvpkg-only-vip-read-only.md)) | only as a detached bundle next to the file |

Rules for every package:

- **One namespace per package.** Everything sits under one top-level `.lvlib`
  (or a library-wrapped class) whose name derives from the package name. Two
  packages that provide the same qualified name are a resolution error.
- **No writes outside the addon.** Machine-level software (NI drivers, a DLL
  in `System32`) is a declared system requirement, not a file group.
- **No install-time code by default.** Palettes, menus and examples come from
  the overlay. A package that needs a hook declares it, and each consumer
  allows it by name.

## 2. Build: `lvpm build`

```toml
[project]
name    = "@hexagon/motion-lib"
version = "2.1.0"                 # SemVer
labview = ">=2021"                # minimum; source packages are saved at its lower bound

[distribution]
top-level-library = "src/Motion.lvlib"  # membership defines the payload, not a folder glob
kinds   = ["source"]              # "ppl" names an existing .lvproj build spec (needs LabVIEW)
```

What `build` does, in order:

1. Collect files by library membership; stray test VIs do not ship.
2. Validate without LabVIEW, using the linker-table reader that
   [`lvpm check`](../roadmap.md#dependency-analysis-lvpm-check--lvpm-tree)
   is planned to bring (a Rust port of `tools/deptree.py`, which is not in
   this repository):
   - every VI is saved at or below the lower bound of `labview`, with compiled code separated;
   - there are no broken links;
   - every external link resolves to `vi.lib`, the package itself, or a declared dependency.
3. Infer dependencies from the links and fail if they disagree with `[dependencies]`.
4. Write a deterministic `.lvpkg` (sorted entries, fixed timestamps) with the SBOM inside.

### No `.vip` output

From S7a, lvpm does not write `.vip` ([ADR 6](../adr/0006-lvpkg-only-vip-read-only.md));
until then `lvpm build` still writes one, for lvpm only.
S1 wrote one, and lvpm installed it, but VIPM refuses it: VIPM checks a
package `ID` in the `spec` that only its own builder writes, and how that
`ID` is computed is not published. A `.vip` that VIPM refuses would only
confuse people, and building through `vipm build` needs VIPM Pro or a public
repository. Library authors who also want VIPM users keep building their
`.vip` with VIPM. The [S1b spec](../specs/lvpm-vip-roundtrip-s1b.md#outcome)
records what was tested.

## 3. Where packages live

There is no single company feed. The design separates the **registry** (which name
belongs to which repo, which hashes are valid) from the **storage** (where the
bytes are). Julia's General registry, Homebrew taps, conda-forge and the
crates.io index are all built this way.

| Source | Storage | Use |
|---|---|---|
| Folder | local disk / UNC share | offline, air-gapped, quick team sharing (`sources.local`, **exists**) |
| VIPM / NI indexes | vipm.io, download.ni.com | the existing ecosystem, read-only (**exists**) |
| OCI registry | ghcr.io, GitLab, Azure Container Registry, Harbor, Artifactory | inner source; content-addressed, standard auth, signatures via the referrers API |
| GitHub Releases, direct | release files of one repo | `libX = { github = "owner/repo", version = ">=1.2" }`, no infrastructure |
| Community index | a git repo of metadata; the files stay on GitHub Releases or OCI | open source discovery and name ownership |

GitHub Packages has no generic or PyPI-style feed, and Azure Artifacts has no OCI
support. Azure DevOps shops either add Azure Container Registry or use a NuGet
feed as a fallback transport.

### GitHub Releases, direct

- Versions come from the release tags.
- The release file is picked by naming convention: `*.lvpkg` first, `*.vip` as fallback
  for releases VIPM built.
- Its SHA-256 is pinned in `lvpm.lock`.
- If the release has a GitHub artifact attestation (Sigstore), lvpm verifies that the
  file was built by that repo's workflow. Otherwise it is trusted on first use and
  pinned in the lock.

### The community index (federated)

- One TOML entry per package:
  - owning repo and maintainers;
  - per version: file URL(s), SHA-256, kind/format, and the expected signer identity.
- Registration is a pull request:
  - a release Action in the package's repo opens it, like Julia's TagBot;
  - a bot checks the name, downloads the file, verifies hash and attestation, and merges.
- Git history is the append-only audit log. Anyone can fork or mirror the index,
  and nobody's outage stops installs.
- Existing `.vip` files on GitHub Releases enter as `format = "vip"` with a hash the
  bot computed. Authors do not have to change anything.
- A deleted release, a re-uploaded file with a different hash, a renamed repo or a
  yank becomes a visible index change instead of silent breakage.
- An organisation runs its own index the same way, or puts a caching proxy in
  front of all of it.

## 4. Consuming: sources, pins, lock, policy

```toml
[sources]                         # searched in this order, then the public defaults
internal  = "oci://<org>.azurecr.io/lv"
community = "git+https://github.com/<org>/lvpm-index"   # placeholder

[dependencies]
"@hexagon/motion-lib" = { version = ">=2.1", index = "internal" }  # only from internal

[policy]
require-signature  = true
trusted-publishers = ["https://github.com/hexagon/*"]
min-release-age    = "3d"         # cf. uv --exclude-newer
```

- Each package resolves from **exactly one** feed: the first source that lists
  it, or the one it pins with `index`. Versions from different feeds are never
  merged, so dependency confusion has nowhere to start
  ([ADR 4](../adr/0004-sources-first-wins-and-pinning.md)).
  The `oci://` and `git+` URLs are the forms S7b and S8 add.
- **One version per name per LabVIEW instance**, because LabVIEW has one global namespace.
- `lvpm.lock` records name, version, source, SHA-256, signer and provenance subject.
  This supersedes the roadmap's MD5-based lock; MD5 is kept only to check
  legacy packages against their index.
  `lvpm install --locked` and `lvpm lock --check` make CI refuse drift.
- Credentials come from the platform's own credential chain
  (`gh auth token`, `az acr login`, `CI_JOB_TOKEN`, Docker credential helpers).
  They never go in `lvpm.toml`.
- Library B develops against A by version, or against `{ path = "../libA" }`
  mounted live, and switches back to a version before release.

## 5. Security baseline

1. SHA-256 everywhere, pinned in the lock. TLS only. MD5 only to read legacy packages.
2. Publishing only from CI, with OIDC-issued short-lived credentials (Trusted Publishing).
3. Signatures:
   - Sigstore keyless where the CI's OIDC issuer is accepted;
   - otherwise Notation with an org key in Key Vault or an HSM.
4. SLSA build provenance attached to the package digest.
5. The consumer's policy decides trust, never the feed.
6. Installs cannot run code or write outside the venv unless explicitly allowed.
7. Published versions are immutable. Yanking hides a version from new resolutions;
   locked installs keep working.
8. Advisories use the OSV format.
9. Shipped DLLs and assemblies are listed in the SBOM, and their Authenticode signatures are optionally checked.

## 6. Legacy bridge

- **VIPM packages:** installable today. Proposed: `lvpm convert x.vip` → `.lvpkg`,
  marked *converted*, hooks kept but disabled, and mirrored and re-signed in an internal
  feed after review.
- **Project files:** `lvpm init --from project.vipc` (or `.dragon`) so existing
  projects can move over. See [vipm-cli-comparison.md](vipm-cli-comparison.md).
- **NIPM:** drivers and toolkits are machine software. They are declared in
  `[nipm.dependencies]` and checked (**exists**). Installing them could be handed to
  `nipkg` as an explicit, machine-wide opt-in.

## References

External facts this design relies on, as of 2026-10:

- GitHub Packages registries (npm, NuGet, Maven, RubyGems, containers; no
  generic feed): <https://docs.github.com/en/packages/working-with-a-github-packages-registry>
- Azure Artifacts feed types: <https://learn.microsoft.com/en-us/azure/devops/artifacts/>
- OCI artifacts with ORAS: <https://oras.land/>
- Registry/storage split: Julia's General registry
  <https://github.com/JuliaRegistries/General>, crates.io index
  <https://doc.rust-lang.org/cargo/reference/registry-index.html>
- Trusted Publishing: <https://docs.pypi.org/trusted-publishers/>
- Sigstore: <https://www.sigstore.dev/>; SLSA provenance: <https://slsa.dev/>
- GitHub artifact attestations: <https://docs.github.com/en/actions/security-for-github-actions/using-artifact-attestations>
- uv `exclude-newer`: <https://docs.astral.sh/uv/reference/settings/#exclude-newer>
- OSV advisory format: <https://ossf.github.io/osv-schema/>

## Open questions

- Who hosts the first community index, and under which organisation.
- Which CI OIDC issuers Sigstore's public instance accepts for Azure Pipelines.
