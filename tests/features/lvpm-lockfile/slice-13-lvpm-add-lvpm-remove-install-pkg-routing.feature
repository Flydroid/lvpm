Feature: add and remove edit the manifest, then lock and sync

  Background:
    Given fixture packages "alpha" 1.0 and 2.0, "beta" 1.0 requiring "alpha" >= 1.0, "gamma" 0.5 and 1.0, "caraya" 1.0
    And a project with labview "2024" depending on "beta" ">=1.0", locked and synced into scratch destination "dest"

  Scenario: add without a version writes a floor of the newest eligible version
    When I run "lvpm add gamma --prefix dest"
    Then lvpm.toml says "gamma" = ">=1.0"
    And lvpm.lock and dest include "gamma" 1.0

  Scenario: add with a constraint writes it as given
    When I run "lvpm add gamma@>=0.5 --prefix dest" (argument quoted)
    Then lvpm.toml says "gamma" = ">=0.5"

  Scenario: add with an empty constraint explains quoting
    When I run "lvpm add gamma@ --prefix dest"
    Then the command fails saying a constraint is expected after "@" and to quote it, e.g. "gamma@>=0.5"
    And it says the shell may have created a file named "=0.5" that can be deleted

  Scenario: add without a lock explains the migration
    Given no lvpm.lock exists and "delta" 1.0 is installed in dest
    When I run "lvpm add gamma --prefix dest"
    Then the output says no lvpm.lock was found, names "delta" as to be removed, and says to commit lvpm.lock

  Scenario: add gated by LabVIEW says why
    Given every "omega" version needs LabVIEW 2026
    When I run "lvpm add omega --prefix dest"
    Then the command fails saying omega requires LabVIEW 2026 and this project is locked for 2024
    And nothing changed

  Scenario: remove --dry-run shows the plan and writes nothing
    Given the project also depends on "gamma" ">=1.0", locked and synced
    When I run "lvpm remove gamma --dry-run --prefix dest"
    Then the output shows the removed manifest line and "remove gamma 1.0"
    And lvpm.toml, lvpm.lock and dest are byte-identical to before
    And nothing changed

  Scenario: add of an existing entry updates its constraint and says so
    When I run "lvpm add beta@>=0.5 --prefix dest"
    Then lvpm.toml says "beta" = ">=0.5" and the output says the constraint was updated

  Scenario: add --dev writes to [dev-dependencies]
    When I run "lvpm add caraya --dev --prefix dest"
    Then [dev-dependencies] says "caraya" = ">=1.0"

  Scenario: add keeps the rest of the manifest byte for byte
    Given lvpm.toml contains comments
    When I run "lvpm add gamma --prefix dest"
    Then lvpm.toml differs from before by exactly the "gamma" line

  Scenario: add --dry-run shows the plan and writes nothing
    When I run "lvpm add gamma --dry-run --prefix dest"
    Then the output shows the manifest line, the lock changes and the sync plan
    And lvpm.toml, lvpm.lock and dest are byte-identical to before

  Scenario: a failed lock during add leaves both files untouched
    Given lvpm.toml pins "alpha" = "1.0" and the only "omega" requires "alpha" >= 2.0
    When I run "lvpm add omega --prefix dest"
    Then the command fails with a conflict naming "alpha" "1.0" and "omega"
    And lvpm.toml and lvpm.lock are byte-identical to before

  Scenario: a failed sync during add leaves both files untouched
    Given the fixture "broken_zip" 1.0 has a valid index entry and a fixed SHA-256, but its archive is not a valid zip
    When I run "lvpm add broken_zip --prefix dest"
    Then the command fails naming "broken_zip" and saying its archive could not be unpacked
    And lvpm.toml and lvpm.lock are byte-identical to before
    And dest holds no "broken_zip" files

  Scenario: remove uninstalls a package nothing else needs
    Given the project also depends on "gamma" ">=1.0", locked and synced
    When I run "lvpm remove gamma --prefix dest"
    Then "gamma" is gone from lvpm.toml, lvpm.lock and dest

  Scenario: remove keeps a package another one still requires
    Given the project also depends on "alpha" ">=1.0", locked and synced
    When I run "lvpm remove alpha --prefix dest"
    Then "alpha" is gone from lvpm.toml
    And "alpha" stays in lvpm.lock and dest
    And the output says "alpha" is still required by "beta"

  Scenario: remove --dev removes from [dev-dependencies]
    Given dev dependency "caraya" ">=1.0", locked and synced
    When I run "lvpm remove caraya --dev --prefix dest"
    Then "caraya" is gone from [dev-dependencies], lvpm.lock and dest

  Scenario: remove without --dev of a dev dependency points to --dev
    Given dev dependency "caraya" ">=1.0"
    When I run "lvpm remove caraya --prefix dest"
    Then the command fails saying "caraya" is in [dev-dependencies] and to use --dev
    And nothing changed

  Scenario: removing the last dependency empties lock and destination
    When I run "lvpm remove beta --prefix dest"
    Then [dependencies] is empty, lvpm.lock lists no packages and dest has no lvpm-installed packages

  Scenario: removing a name that is not a dependency fails
    When I run "lvpm remove zeta --prefix dest"
    Then the command fails saying "zeta" is not in lvpm.toml
    And nothing changed

  @core
  Scenario: a failed sync during remove leaves both files untouched
    Given an OpEnv whose install session fails when it uninstalls "beta"
    When core removes "beta"
    Then the result is a sync error naming "beta"
    And lvpm.toml and lvpm.lock are byte-identical to before

  # "no project" is written as the directory it is, which an earlier step
  # already names.
  @core
  Scenario Outline: install routing follows the table
    Given <project>
    When core routes "lvpm install <args>"
    Then the mode is <mode>

    Examples:
      | project                        | args                         | mode                              |
      | a project                      | gamma                        | Add                               |
      | a project                      | gamma --prefix dest          | Additive                          |
      | a project                      | gamma --global               | Additive                          |
      | a project                      | gamma --labview-version 2026 | Additive                          |
      | a directory without lvpm.toml  | gamma                        | Additive                          |
      | a project                      | (none)                       | Sync                              |
      | a project                      | --prefix dest                | Sync                              |
      | a project                      | --global                     | Sync                              |
      | a project                      | --no-deps                    | error "not supported with a lock" |
      | a directory without lvpm.toml  | (none)                       | error "give a package name"       |

  @labview
  Scenario: install with a name in a project is add, and says so
    Given a project with a venv
    When I run "lvpm install gamma", and "lvpm add gamma" in an identical world
    Then the first output line says gamma is being added to lvpm.toml and the project synced to lvpm.lock
    And both manifests, locks and destinations are identical

  Scenario Outline: install with a name and an explicit destination stays additive
    When I run "lvpm install gamma <flag>" inside the project
    Then gamma is installed in that destination
    And lvpm.toml and lvpm.lock are byte-identical to before

    Examples:
      | flag          |
      | --prefix dest |

  Scenario: install flags that make no sense with a lock are rejected
    When I run "lvpm install --no-deps" inside the project
    Then the command fails saying --no-deps is not supported with a lock

  Scenario: uninstall inside a project hints at remove
    When I run "lvpm uninstall beta --prefix dest"
    Then "beta" is uninstalled
    And the output says "beta" is still in lvpm.toml and "lvpm remove beta" removes it for good

  Scenario Outline: lock-editing commands outside a project are refused
    Given a directory without lvpm.toml
    When I run "<command>"
    Then the command fails saying no lvpm.toml was found, at or above the current directory

    Examples:
      | command          |
      | lvpm add gamma   |
      | lvpm remove beta |
      | lvpm lock         |
      | lvpm lock --check |
      | lvpm sync         |
      | lvpm install      |

  # The recording was made with nothing of a project about, so the world is
  # emptied to match: the Background's packages, lock and destination would
  # change what is printed.
  Scenario: install with a name outside a project is unchanged
    Given a directory without lvpm.toml, no local packages and no dest, with the default feeds cached
    When I run "lvpm install beta --prefix dest"
    Then the output equals the recording "install-beta-noproject"

  # Never run: this installs into the LabVIEW installation itself, which no
  # scenario may touch (see `real_installation_risk`). Which destination
  # `--global` and `--labview-version` mean is covered by the routing table
  # above, and the unit tests of `install_mode`.
  @labview @unsafe
  Scenario: install with a name and --global stays additive
    When I run "lvpm install gamma --global --labview-version 2026" inside the project
    Then gamma is installed into that LabVIEW and lvpm.toml and lvpm.lock are unchanged
