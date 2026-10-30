# lvpm

An open-source package manager for LabVIEW.

lvpm installs `.vip` and `.ogp` packages, the formats VIPM and OpenG use, and
its own `.lvpkg`, by name. It takes them from the public package directories,
your own hosted indexes, or local folders, and installs them into a LabVIEW
installation or into a project's own venv, so two projects on one machine can
use different versions of the same library.

Not affiliated with JKI or NI. Working, in active development; the
[roadmap](https://github.com/ThatsTheEnd/lvpm/blob/main/docs/roadmap.md) has
what is planned, the
[README](https://github.com/ThatsTheEnd/lvpm#known-limitations) the known
limitations.

{{#include ../README.md:how-it-works}}

## How this book is organised

- **[Tutorial](tutorial/)**: start here. From an empty folder to a
  project with dependencies, then to a package of your own, one step at a
  time.
- **How-to guides**: one task each, for when you know what you want:
  [installing lvpm](install.md), [running it in CI](ci-docker.md).
- **Reference**: every [command, flag and manifest key](usage.md), the
  [JSON output and exit codes](reference/json-output.md).
- **Explanation**: why lvpm works the way it does: the
  [OpenG package model](ogpm-model.md) it implements,
  [what an install does inside LabVIEW](labview-integration.md), and the
  [architecture decisions](adr/).
