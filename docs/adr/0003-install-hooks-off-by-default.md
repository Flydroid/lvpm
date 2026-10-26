# 3. Install hooks are off by default, with a warning and an allowlist

Date: 2026-10-26

## Status

Accepted

## Context

A `.vip` can declare `[Script VIs]`: VIs that run inside LabVIEW, with the
user's rights, when the package is installed or uninstalled. 487 of 1,826
specs in a local VIPM cache (27%) declare at least one; `PostInstall` is the
most common (451).

Code that runs at install time is a well-known supply-chain attack route. It
runs before anyone has looked at the package, for every transitive dependency,
often on a machine that holds credentials. npm runs `postinstall` by default
and has had large incidents through it (ua-parser-js in 2021, the Shai-Hulud
worm in 2025). pnpm (v10) and Bun now skip dependency scripts unless the
package is on an allowlist. Go modules and NuGet's `PackageReference` run no
install code at all.

lvpm today:

- in a venv, hooks never run, because they act on a LabVIEW installation and
  a venv is not one;
- headless (`LV_RTE_HEADLESS` set), hooks are skipped unless `--hooks` is given;
- on a developer machine, a global install runs `PostInstall`. This is npm's
  default.

`lvpm list` already shows skipped hooks, and `lvpm run-hooks` runs them later.

## Decision

1. **Hooks are off by default everywhere**, developer machines included.
   Venvs never run them, as today.
2. **A skipped hook is never silent.** After any install or sync that skipped
   one, lvpm prints a warning naming the package and the hook. The warning
   says how to run it now, with `lvpm run-hooks <package>`, and how to allow
   it on every install. It is a warning, not an error: the install still
   succeeds and the exit code stays 0.
3. **Hooks are allowed per package, by name.**
   - In a project, the allowlist lives in `lvpm.toml` and is committed, so
     everyone on the team gets the same behaviour. Only the root project's
     manifest counts; a dependency's own manifest can't allow hooks.
   - For a global install without a project, it lives in lvpm's user config.
   - `--hooks` still allows every hook for that one command.
4. **Uninstall hooks follow the install hook.** A package whose install hook
   did not run does not run its uninstall hooks either.

## Consequences

- This changes shipped behaviour: a global install that ran `PostInstall`
  before now skips it and warns. Packages that need their hook (template
  installers, menu entries) need one allowlist line.
- Slice S12 builds it. The project allowlist is `hooks = "run"` on the
  dependency line ([ADR 4](0004-sources-first-wins-and-pinning.md)). The
  user-config key and the warning's wording are decided in S12's `/specs`.
- An allowlist by name trusts every future version of that package, as pnpm's
  and Bun's do. This is a choice: the package file's SHA-256 in `lvpm.lock`
  already stops a swapped file under a locked version, and the upgrade shows
  in the lock diff. Pinning each hook VI's hash would ask again on almost
  every upgrade, since VIs change bytes on any resave. It can be added later
  without breaking anything.
