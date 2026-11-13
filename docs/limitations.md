# Known limitations

- `Replace Mode = If Newer` only writes when the file is absent; doing it
  properly means comparing archive timestamps against disk.
- No dependency conflict resolution: each package gets the newest version
  meeting the highest floor anything asks for, and a manifest pin always
  wins (with a warning when a dependent asks for more).
- No mass compile; LabVIEW recompiles installed VIs on first load.
- A hook VI's own `error out` is not read back. A hook that fails internally
  but runs to completion reports as ok.
- A failure part-way through unpacking leaves files behind with no manifest,
  and one failed download aborts the rest of the plan.
- Windows is the tested platform. Linux is verified against LabVIEW 2026 Q3
  in NI's container image only, and does not yet honour a package's
  `Exclusive_OS` ([linux.md](linux.md)).
- In a venv, `PostInstall` hooks run in the venv's own LabVIEW but
  `PreInstall` ones do not (that instance only starts once the files are
  down), and file groups aimed at machine locations (`<OS ...>`, `<temp>`) are
  skipped — both are recorded in the manifest and shown by `lvpm list`. A hook
  that drops palette files into the overlay is picked up at the next
  `lvpm launch`, not by a refresh. Uninstall hooks still never run in a venv,
  whose LabVIEW has to be closed before its files can be removed.
  `lvpm install <name>` in a venv does not
  add the pin to `lvpm.toml`. No warning yet when a global copy of a venv
  package shadows it.
- `lvpm launch` checks install records, not file contents: a package VI
  edited or resaved since install is not detected.

[roadmap.md](roadmap.md) has what is planned.
