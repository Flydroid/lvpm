@core
Feature: The solver finds a set that satisfies every constraint or explains why none exists

  Scenario: it finds a set where greedy picking fails
    Given the catalog has "A" 1.0 requiring "C" >= 1.0, "A" 2.0 requiring "C" >= 2.0, and "C" 1.0 and 2.0
    And the constraints are "A" "*" and "C" "1.0"
    When the solver runs for LabVIEW 2024
    Then the resolution is exactly "A" 1.0 and "C" 1.0

  Scenario: it explains a conflict in terms of the constraints
    Given the catalog has only "A" 2.0 requiring "C" >= 2.0, and "C" 1.0
    And the constraints are "A" "*" and "C" "1.0"
    When the solver runs
    Then the result is a conflict whose report contains "A", "C >= 2.0" and "C = 1.0"

  Scenario Outline: the LabVIEW gate
    Given "alpha" 2.0 needs LabVIEW <alpha2> and "alpha" 1.0 needs LabVIEW 2020
    And the constraints are "alpha" "*"
    When the solver runs for LabVIEW 2024
    Then the resolution holds "alpha" <chosen>

    Examples:
      | alpha2 | chosen |
      | 2026   | 1.0    |
      | 2024   | 2.0    |

  Scenario: the gate also applies to transitive packages
    Given "beta" 1.0 requires "alpha" >= 1.0, and "alpha" 2.0 needs LabVIEW 2026 and "alpha" 1.0 needs LabVIEW 2020
    And the constraints are "beta" "*"
    When the solver runs for LabVIEW 2024
    Then the resolution holds "alpha" 1.0

  Scenario: preferred versions are kept when they still fit
    Given the catalog has "alpha" 1.0 and 2.0
    And the constraints are "alpha" ">=1.0" and the preference is "alpha" 1.0
    When the solver runs
    Then the resolution holds "alpha" 1.0

  Scenario: a requirement no source lists is unresolved, not a failure
    Given "beta" 1.0 requires "ni_lib_missing" >= 1.0, which no source lists
    And the constraints are "beta" "*"
    When the solver runs
    Then the resolution holds "beta" 1.0
    And "ni_lib_missing" is reported unresolved, required by "beta"

  Scenario: a direct dependency no source lists is a conflict
    Given the constraints are "zeta" "*", which no source lists
    When the solver runs
    Then the result is a conflict naming "zeta" as not found

  Scenario: names match case-insensitively
    Given the catalog has "Alpha" 1.0 and "beta" 1.0 requiring "alpha" >= 1.0
    And the constraints are "beta" "*"
    When the solver runs
    Then the resolution holds one entry for "alpha" and one for "beta"

  Scenario: a cancelled token stops the solver
    Given a cancelled token
    When the solver runs
    Then the result is "cancelled"
