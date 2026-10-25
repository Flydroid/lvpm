Feature: lvpm sync makes the destination equal the lock

  Background:
    Given fixture packages "alpha" 1.0 and 2.0, "beta" 1.0 requiring "alpha" >= 1.0, "gamma" 1.0, "caraya" 1.0 requiring "delta" >= 1.0, "delta" 1.0
    And a project with labview "2024" depending on "beta" ">=1.0", with a scratch destination "dest"

  Scenario: sync installs exactly the locked set
    Given a current lvpm.lock with "alpha" 2.0 and "beta" 1.0
    When I run "lvpm sync --prefix dest"
    Then dest's lvpm-installed packages are exactly "alpha" 2.0 and "beta" 1.0
    And each install manifest's archive SHA-256 equals the locked one

  Scenario: an extra package is announced, then removed, without a prompt
    Given a current lvpm.lock and "gamma" 1.0 installed in dest
    When I run "lvpm sync --prefix dest" with no input available
    Then the line naming "gamma" as removed because lvpm.lock does not list it comes before the removal output
    And "gamma" is no longer installed
    And the command did not wait for input

  Scenario: a package the lock lists is never removed
    Given a current lvpm.lock with "alpha" 2.0 and "beta" 1.0, both installed in dest
    When I run "lvpm sync --prefix dest"
    Then the output says nothing to do
    And nothing is installed or removed

  Scenario: a different installed version is replaced
    Given "alpha" 1.0 installed in dest and lvpm.lock holds "alpha" 2.0
    When I run "lvpm sync --prefix dest"
    Then "alpha" 2.0 is installed and the output names "alpha 1.0 -> 2.0"

  Scenario: dry-run shows the plan and changes nothing
    Given lvpm.lock holds "alpha" 2.0 and "beta" 1.0, lvpm.toml now also depends on "gamma" "*", and "delta" 1.0 is installed in dest
    When I run "lvpm sync --dry-run --prefix dest"
    Then the output says lvpm.lock would be updated: "+ gamma 1.0"
    And it lists "install alpha 2.0", "install beta 1.0", "install gamma 1.0" and "remove delta 1.0 (not in lvpm.lock)"
    And dest, lvpm.lock and lvpm.toml are byte-identical to before

  Scenario: the first sync of a project explains the migration
    Given no lvpm.lock exists and "gamma" 1.0 installed in dest
    When I run "lvpm sync --prefix dest"
    Then the output says no lvpm.lock was found and one is being written
    And it says 1 installed package not in the lock will be removed, naming "gamma"
    And it says to commit lvpm.lock
    And dest matches the new lvpm.lock

  Scenario: a stale lock is re-locked before syncing
    Given a lvpm.lock without "gamma" and lvpm.toml now also depends on "gamma" "*"
    When I run "lvpm sync --prefix dest"
    Then lvpm.lock now holds "gamma" 1.0 and dest has "gamma" installed

  Scenario: a package whose recorded hosted source is gone comes from a local folder
    Given lvpm.lock records "alpha" 2.0 with the loopback feed as its source, and the cache is empty
    And the loopback feed is stopped and a local source folder holds the identical archive
    When I run "lvpm sync --prefix dest"
    Then "alpha" 2.0 is installed with the locked SHA-256

  Scenario: a local package found in a different local folder is accepted
    Given lvpm.lock records "alpha" 2.0 with source "local", and the cache is empty
    And the original fixture folder is removed and a second configured local folder holds the identical archive
    When I run "lvpm sync --prefix dest"
    Then "alpha" 2.0 is installed with the locked SHA-256

  Scenario: an imported archive needs no source
    Given "alpha" 2.0 was imported with "lvpm cache add" and no source holds it
    When I run "lvpm sync --prefix dest"
    Then "alpha" 2.0 is installed

  Scenario: a missing archive fails before any change
    Given a current lvpm.lock with "alpha" 2.0 and "beta" 1.0, "gamma" 1.0 installed in dest
    And no source or cache holds an archive with the locked SHA-256 of "alpha" 2.0
    When I run "lvpm sync --prefix dest"
    Then the command fails naming "alpha", "2.0" and the SHA-256
    And it suggests "lvpm cache add <file>" or a sources.local folder
    And "gamma" is still installed and "beta" is not installed

  Scenario Outline: an archive with different bytes is never installed
    Given <where> holds an "alpha" 2.0 archive whose bytes differ from the locked SHA-256, and nothing else holds it
    When I run "lvpm sync --prefix dest"
    Then the command fails saying the archive of "alpha" 2.0 does not match lvpm.lock
    And "alpha" 2.0 is not installed

    Examples:
      | where              |
      | the fixture folder |
      | the cache          |

  Scenario: --locked refuses a stale lock
    Given a stale lvpm.lock
    When I run "lvpm sync --locked --prefix dest"
    Then the command fails with the stale reason and "run lvpm lock and commit lvpm.lock"
    And dest and lvpm.lock are unchanged

  Scenario: --locked refuses a missing lock
    Given no lvpm.lock exists
    When I run "lvpm sync --locked --prefix dest"
    Then the command fails and no lvpm.lock is written

  Scenario: --frozen syncs a stale lock as it is
    Given lvpm.lock holds "alpha" 2.0 and "beta" 1.0 and lvpm.toml now also depends on "gamma" "*"
    When I run "lvpm sync --frozen --prefix dest"
    Then dest has exactly "alpha" 2.0 and "beta" 1.0
    And lvpm.lock is byte-identical to before

  Scenario: --frozen without a lock fails
    Given no lvpm.lock exists
    When I run "lvpm sync --frozen --prefix dest"
    Then the command fails saying there is no lvpm.lock

  Scenario: --locked and --frozen together are rejected
    When I run "lvpm sync --locked --frozen"
    Then the command fails saying the two flags cannot be combined

  Scenario: --no-dev skips dev-only packages, transitive ones included
    Given dev dependency "caraya" "*" and a current lock, with "caraya" and "delta" installed in dest
    When I run "lvpm sync --no-dev --prefix dest"
    Then "caraya" and "delta" are removed
    And "alpha" and "beta" are installed

  Scenario: a package needed by both normal and dev dependencies stays with --no-dev
    Given "caraya" also requires "alpha" >= 1.0 and a current lock
    When I run "lvpm sync --no-dev --prefix dest"
    Then "alpha" is installed

  Scenario Outline: bare install is sync with the same flags
    When I run "lvpm install <flags>" and, in an identical world, "lvpm sync <flags>"
    Then both destinations, locks and normalised outputs are identical

    Examples:
      | flags                     |
      | --prefix dest             |
      | --dry-run --prefix dest   |
      | --locked --prefix dest    |
      | --frozen --prefix dest    |
      | --no-dev --prefix dest    |

  Scenario: an unresolved requirement is warned about at sync
    Given lvpm.lock records "ni_lib_missing" as unresolved, required by "beta"
    When I run "lvpm sync --prefix dest"
    Then the output warns that "ni_lib_missing", required by "beta", is not installed

  Scenario: sync with an unsatisfiable manifest changes nothing
    Given lvpm.lock is stale and lvpm.toml pins "alpha" = "1.0" while "beta" 1.0 now requires "alpha" >= 2.0
    When I run "lvpm sync --prefix dest"
    Then the command fails with the conflict report
    And dest and lvpm.lock are unchanged

  Scenario: sync outside a project is refused
    Given a directory without lvpm.toml
    When I run "lvpm sync"
    Then the command fails saying no lvpm.toml was found

  Scenario: a manifest without labview is refused at sync with an example
    Given a project manifest without labview
    When I run "lvpm install --prefix dest"
    Then the command fails with: add labview = "<year>" under [project] in lvpm.toml

  # Three scenarios in this file (the hook one, --global and headless) are
  # tagged @unsafe and never run. A sync takes out every package its lock does
  # not list, and they would run one against the user's own LabVIEW
  # installation: the harness refuses (see `real_installation_risk` in
  # tests/support/sandbox.rs). The hook scenario needs a PostInstall hook to
  # fail, and a venv runs none, so it can only happen on the installation too.
  # What they describe is checked by the scenarios that sync into --prefix
  # trees, and by "sync into a venv". The --global and headless destinations
  # run in core's unit tests against a fake installation (a_sync_with_global_is_
  # exact_on_the_installation and a_headless_sync_..._is_exact_on_the_installation
  # in ops/sync.rs).
  @labview @unsafe
  Scenario: a hook failing mid-sync tells how to finish
    Given a lock with "hooked" whose PostInstall fails, in a venv
    When I run "lvpm sync"
    Then the output lists what was done and what failed, ending with "re-run lvpm sync to finish"
    And a second "lvpm sync" reports nothing left to do except the hook

  @labview
  Scenario: sync into a venv
    Given a project with a venv and a current lock
    When I run "lvpm sync"
    Then the venv's lvpm-installed packages equal the lock

  @labview @unsafe
  Scenario: sync with --global on a workstation is exact
    Given the LabVIEW installation has an lvpm-installed package the lock does not list
    When I run "lvpm sync --global"
    Then that package is uninstalled and the locked set is installed

  @labview @unsafe
  Scenario: headless sync into the manifest's LabVIEW is exact
    Given LV_RTE_HEADLESS is set, no venv, a current lock
    And that LabVIEW has an lvpm-installed package the lock does not list
    When I run "lvpm sync"
    Then that package is uninstalled and the locked set is installed
