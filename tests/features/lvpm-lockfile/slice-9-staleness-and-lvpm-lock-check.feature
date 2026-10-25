Feature: lvpm knows when the lock no longer represents the manifest

  Background:
    Given a project with labview "2024", defaults = false, depending on "beta" ">=1.0", locked with "alpha" 1.0 and "beta" 1.0

  Scenario Outline: a manifest change makes the lock stale and check says why
    When I <change>
    And I run "lvpm lock --check"
    Then the exit code is non-zero
    And stderr is one line naming <reason>
    And stdout is empty
    And no file in the project changed

    Examples:
      | change                                      | reason                          |
      | add the dependency "gamma" "*"              | dependency "gamma" was added    |
      | remove the dependency "beta"                | dependency "beta" was removed   |
      | add the dev dependency "caraya" "*"         | dev dependency "caraya" was added |
      | change "beta" to "2.0"                      | "beta" "2.0" excludes locked 1.0 |
      | change labview to "2025"                    | labview changed                 |
      | add a hosted source to [sources]            | sources changed                 |
      | set defaults = true                         | sources changed                 |

  Scenario Outline: some changes keep the lock current
    When I <change>
    And I run "lvpm lock --check"
    Then the exit code is 0 and stderr is empty

    Examples:
      | change                                                 |
      | relax "beta" to "*"                                    |
      | add a comment and reorder [dependencies]               |
      | change [project] name                                  |
      | add a folder to sources.local                          |
      | publish "beta" 3.0 to the fixture folder               |

  Scenario: check without a lock fails and says so
    Given lvpm.lock is deleted
    When I run "lvpm lock --check"
    Then the exit code is non-zero and stderr says there is no lvpm.lock and to run "lvpm lock"

  Scenario: an unreadable lock is reported, not overwritten silently
    Given lvpm.lock is not valid TOML
    When I run "lvpm lock --check"
    Then the exit code is non-zero and stderr says lvpm.lock cannot be read and to run "lvpm lock" to regenerate it

  Scenario: check sends no network request
    Given the project lists the loopback feed under [sources] and the feed is stopped
    When I run "lvpm lock --check"
    Then the exit code is 0

  Scenario: a lock in an older format is stale
    Given lvpm.lock declares lock-format version 0
    When I run "lvpm lock --check"
    Then the exit code is non-zero and stderr says the lock format is outdated

  Scenario: a current lock is not rewritten
    When I run "lvpm lock"
    Then the output says lvpm.lock is up to date
    And lvpm.lock's bytes and modification time are unchanged

  Scenario: a new upstream release changes nothing
    Given "beta" 3.0 is published to the fixture folder
    When I run "lvpm lock"
    Then lvpm.lock still holds "beta" 1.0
