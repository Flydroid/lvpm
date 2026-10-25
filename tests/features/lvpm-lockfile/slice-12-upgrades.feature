Feature: Only an explicit upgrade moves locked versions

  Background:
    Given fixture packages "alpha" 1.0 and 2.0, "beta" 0.9, 1.0, 1.5 and 2.0 each requiring "alpha" >= 1.0, "gamma" 1.0 and 2.0
    And a project with labview "2024" depending on "beta" ">=1.0" and "gamma" "1.0"
    And lvpm.lock holds "alpha" 1.0, "beta" 1.0 and "gamma" 1.0

  Scenario: upgrade everything within the constraints
    When I run "lvpm lock --upgrade"
    Then lvpm.lock holds "alpha" 2.0, "beta" 2.0 and "gamma" 1.0
    And the output lists "alpha 1.0 -> 2.0" and "beta 1.0 -> 2.0"

  Scenario: upgrade never crosses the LabVIEW gate
    Given "alpha" 3.0 needing LabVIEW 2026 is published
    When I run "lvpm lock --upgrade"
    Then lvpm.lock holds "alpha" 2.0

  Scenario: upgrade one package and keep the rest
    When I run "lvpm lock --upgrade-package beta"
    Then "beta" is 2.0 in lvpm.lock
    And the "alpha" and "gamma" entries are byte-identical to before

  Scenario: upgrading one package moves a dependency only when it must
    Given "beta" 2.0 requires "alpha" >= 2.0 instead
    When I run "lvpm lock --upgrade-package beta"
    Then "beta" is 2.0 and "alpha" is 2.0 in lvpm.lock
    And the "gamma" entry is byte-identical to before

  Scenario: upgrade-package is repeatable
    When I run "lvpm lock --upgrade-package beta --upgrade-package alpha"
    Then "beta" is 2.0 and "alpha" is 2.0, and "gamma" is unchanged

  Scenario: a version the constraint admits leaves the manifest alone
    When I run "lvpm lock --upgrade-package beta@1.5"
    Then "beta" is 1.5 in lvpm.lock
    And lvpm.toml is byte-identical to before

  Scenario: a version the floor excludes rewrites the floor and says so
    When I run "lvpm lock --upgrade-package beta@0.9"
    Then lvpm.toml says "beta" = ">=0.9" and nothing else in it changed
    And the output says lvpm.toml was updated: beta ">=1.0" -> ">=0.9"
    And "beta" is 0.9 in lvpm.lock

  Scenario: a version an exact pin excludes rewrites the pin
    When I run "lvpm lock --upgrade-package gamma@2.0"
    Then lvpm.toml says "gamma" = "2.0"

  Scenario: a "*" constraint is never rewritten
    Given lvpm.toml says "beta" = "*"
    When I run "lvpm lock --upgrade-package beta@0.9"
    Then lvpm.toml still says "beta" = "*" and "beta" is 0.9 in lvpm.lock

  Scenario: a transitive package is never added to the manifest
    When I run "lvpm lock --upgrade-package alpha@2.0"
    Then "alpha" is 2.0 in lvpm.lock
    And lvpm.toml has no "alpha" entry

  Scenario: a version that does not exist fails and changes nothing
    When I run "lvpm lock --upgrade-package beta@9.9"
    Then the command fails saying no "beta" 9.9 is in any source
    And lvpm.toml and lvpm.lock are byte-identical to before

  Scenario: a name that is not locked fails and changes nothing
    When I run "lvpm lock --upgrade-package zeta"
    Then the command fails saying "zeta" is not in lvpm.lock
    And lvpm.toml and lvpm.lock are byte-identical to before

  Scenario Outline: an upgrade to an ineligible version fails and changes nothing
    Given <situation>
    When I run "lvpm lock --upgrade-package beta@2.0"
    Then the command fails saying <reason>
    And lvpm.toml and lvpm.lock are byte-identical to before

    Examples:
      | situation                                                          | reason                                                          |
      | "beta" 2.0 needing LabVIEW 2026 is published                       | beta 2.0 requires LabVIEW 2026; this project is locked for 2024 |
      | lvpm.toml pins "alpha" = "1.0" and beta 2.0 requires alpha >= 2.0  | the conflict, naming "alpha" "1.0"                              |

  Scenario: nothing newer says so
    Given lvpm.lock already holds the newest eligible versions
    When I run "lvpm lock --upgrade"
    Then the output says no changes, nothing newer
