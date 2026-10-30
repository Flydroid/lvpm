# Your first project

This tutorial takes you from an empty folder to a LabVIEW project whose
dependencies live in the project, and then to a package of your own that
another project can install. Work through it in order; each page builds on
the one before.

1. [A project and its venv](project-and-venv.md): write `lvpm.toml`, create
   the venv.
2. [Add a dependency, open LabVIEW](add-and-launch.md): `lvpm add`,
   `lvpm launch`.
3. [Package and publish a library](build-and-publish.md): `lvpm build`,
   `lvpm publish`.

You need:

- lvpm on your `PATH` ([Install lvpm](../install.md)). `lvpm --version`
  answers.
- LabVIEW 2024 Q1 or later on Windows. The tutorial uses LabVIEW 2026; put
  your own version wherever it says `2026`. `lvpm targets` lists the
  installations lvpm sees.
- LabVIEW's VI Server (TCP) enabled, which is its default
  (Tools » Options » VI Server).
- Network access to the public package indexes.
