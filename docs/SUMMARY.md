# Summary

[Introduction](introduction.md)

# Tutorial

- [Your first project](tutorial/README.md)
  - [1. A project and its venv](tutorial/project-and-venv.md)
  - [2. Add a dependency, open LabVIEW](tutorial/add-and-launch.md)
  - [3. Package and publish a library](tutorial/build-and-publish.md)

# How-to guides

- [Install lvpm](install.md)
- [Run lvpm in CI (Docker)](ci-docker.md)

# Reference

- [Commands and configuration](usage.md)
- [JSON output and exit codes](reference/json-output.md)
- [The VI Server protocol](vi-server-protocol.md)

# Explanation

- [The OpenG package model](ogpm-model.md)
- [What an install does in LabVIEW](labview-integration.md)
- [Architecture decisions](adr/README.md)
  - [1. Manifest: project and distribution tables](adr/0001-manifest-project-and-distribution.md)
  - [2. .vip first, .lvpkg later (superseded)](adr/0002-vip-first-lvpkg-later.md)
  - [3. Install hooks off by default](adr/0003-install-hooks-off-by-default.md)
  - [4. Sources: the first one wins](adr/0004-sources-first-wins-and-pinning.md)
  - [5. Core does the work, the CLI the terminal](adr/0005-core-and-cli-split.md)
  - [6. Write .lvpkg only, read .vip](adr/0006-lvpkg-only-vip-read-only.md)
