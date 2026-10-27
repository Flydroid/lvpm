Feature: Project mode follows the lock

  Background:
    Given the TUI runs at 80x24 in a project with a venv
    And lvpm.toml has a comment and lists "alpha" = ">=1.0"
    And the lock holds "alpha" 1.0 and "beta" 1.0, which "alpha" requires, both installed in the venv
    And a source with "alpha" 1.0, "beta" 1.0 and "gamma" 1.2; "the catalog has X" adds X to it

  Scenario: install from the list adds a floor, locks and syncs
    When "gamma" 1.2 is selected and "i" is pressed
    Then the preview is titled for the project and lists "gamma" 1.2 as new
    And it says lvpm.toml gains "gamma" = ">=1.2" and lvpm.lock changes
    And nothing is written yet
    When Enter is pressed
    Then lvpm.toml lists "gamma" = ">=1.2" and keeps its comment
    And lvpm.lock holds "gamma" 1.2
    And "gamma" 1.2 is installed in the venv

  Scenario: an add that cannot be resolved changes nothing
    Given "gamma" 1.2 instead requires "omega" 1.0, which no source has
    When "gamma" is selected and "i" is pressed
    Then the screen shows core's error naming "omega"
    And lvpm.toml and lvpm.lock are byte-identical

  Scenario: uninstall of a direct dependency removes it from the manifest
    When "alpha" is selected and "x" is pressed
    Then the preview says lvpm.toml loses "alpha" and lists "alpha" and "beta" to remove from the venv
    When "y" is pressed
    Then lvpm.toml no longer lists "alpha"
    And "alpha" and "beta" are no longer installed in the venv

  Scenario: a removed direct dependency still required stays as transitive
    Given lvpm.toml instead lists "alpha" = ">=1.0" and "beta" = ">=1.0"
    When "beta" is selected and "x" is pressed
    Then the preview says "beta" stays, required by "alpha"
    When "y" is pressed
    Then lvpm.toml no longer lists "beta"
    And lvpm.lock and the venv still hold "beta" 1.0

  Scenario: uninstall is refused for a transitive dependency
    When "beta" is selected and "x" is pressed
    Then the screen says "beta" is required by "alpha" and nothing runs

  Scenario: an upgrade inside the constraint changes only the lock
    Given the catalog has "alpha" 2.0
    When "alpha" is selected, "u" is pressed and Enter is pressed
    Then lvpm.lock holds "alpha" 2.0 and lvpm.toml is byte-identical
    And "alpha" 2.0 is installed in the venv

  Scenario: an exact pin the new version breaks is rewritten as an exact pin
    Given lvpm.toml instead lists "alpha" = "1.0"
    And the catalog has "alpha" 2.0
    When "alpha" is selected, "u" is pressed and Enter is pressed
    Then lvpm.toml lists "alpha" = "2.0"
    And "alpha" 2.0 is installed in the venv

  Scenario: a downgrade below the floor lowers the floor
    Given the catalog has "alpha" 0.9
    When version 0.9 of "alpha" is picked in the details, "i" is pressed, Enter is pressed and "y" is pressed
    Then lvpm.toml lists "alpha" = ">=0.9"
    And "alpha" 0.9 is installed in the venv

  Scenario: a "*" constraint is never rewritten
    Given lvpm.toml instead lists "alpha" = "*"
    And the catalog has "alpha" 2.0
    When "alpha" is selected, "u" is pressed and Enter is pressed
    Then lvpm.toml is byte-identical and "alpha" 2.0 is installed in the venv

  Scenario: an upgrade of a transitive dependency never adds it to the manifest
    Given the catalog has "beta" 2.0
    When "beta" is selected, "u" is pressed and Enter is pressed
    Then lvpm.toml does not list "beta"
    And "beta" 2.0 is installed in the venv

  Scenario: upgrade all leaves a floor-only manifest byte-identical
    Given the catalog has "alpha" 2.0 and "beta" 2.0
    When "U" is pressed and Enter is pressed
    Then lvpm.lock holds both at 2.0 and lvpm.toml is byte-identical
    And both are installed at 2.0 in the venv

  Scenario: upgrade all keeps an exact pin
    Given lvpm.toml instead lists "alpha" = "1.0"
    And the catalog has "alpha" 2.0 and "beta" 2.0
    When "U" is pressed and Enter is pressed
    Then lvpm.lock holds "alpha" 1.0 and "beta" 2.0
    And lvpm.toml is byte-identical

  Scenario: a multi-select upgrade is one lock upgrade and one sync
    Given the catalog has "alpha" 2.0 and "beta" 2.0
    When "alpha" and "beta" are marked and "u" is pressed
    Then one preview lists both
    When Enter is pressed
    Then the result lists one sync with both packages
    And both are installed at 2.0 in the venv

  Scenario: a sync that fails after the lock is written
    Given the catalog has "alpha" 2.0 whose archive is corrupt
    When "alpha" is selected, "u" is pressed and Enter is pressed
    Then the result shows core's error naming "alpha" 2.0
    And it says lvpm.lock was updated but the venv was not synced
    And the list shows "alpha" 1.0 installed in the venv
