# Architecture decisions

Each record states one decision, the context it was made in and what follows
from it. A superseded record stays, with a pointer to the one that replaced
it.

| ADR | Decision | Status |
| --- | --- | --- |
| [1](0001-manifest-project-and-distribution.md) | `lvpm.toml`: one `[project]`, a `[distribution]` table, LabVIEW builds stay in the `.lvproj` | Accepted, amended by 6 |
| [2](0002-vip-first-lvpkg-later.md) | `.vip` first, `.lvpkg` later | Superseded by 6 |
| [3](0003-install-hooks-off-by-default.md) | Install hooks are off by default, with a warning and an allowlist | Accepted |
| [4](0004-sources-first-wins-and-pinning.md) | Sources: the first one wins, a dependency can pin one | Accepted |
| [5](0005-core-and-cli-split.md) | Core does the work, the CLI does the terminal | Accepted |
| [6](0006-lvpkg-only-vip-read-only.md) | lvpm writes `.lvpkg` only; `.vip` is read, never written | Accepted |
