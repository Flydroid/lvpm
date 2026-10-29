Feature: a package name comes only from the first source that lists it

  Background:
    Given a project with labview "2024" whose [sources] are "first" then "second"

  Scenario: the first source's copy is locked although a later source lists a higher version
    Given "first" lists "alpha" 1.0
    And "second" lists "alpha" 2.0
    And the project depends on "alpha" "*"
    When I run "lvpm lock"
    Then the command succeeds
    And lvpm.lock takes "alpha" 1.0 from "first"

  Scenario: a name only the second source lists is locked from it
    Given "first" lists "alpha" 1.0
    And "second" lists "gamma" 1.0
    And the project depends on "gamma" "*"
    When I run "lvpm lock"
    Then the command succeeds
    And lvpm.lock takes "gamma" 1.0 from "second"

  Scenario: a version the first source does not list fails naming it, not falling through
    Given "first" lists "alpha" 1.0
    And "second" lists "alpha" 2.0
    And the project depends on "alpha" ">=2.0"
    When I run "lvpm lock"
    Then the command fails naming "first"
    And no lvpm.lock is written

  Scenario: a version the first source lists only for a newer LabVIEW fails naming it
    Given "first" lists "alpha" 3.0
    And "second" lists "alpha" 2.0
    And the project depends on "alpha" ">=2.0"
    When I run "lvpm lock"
    Then the command fails naming "first"
    And no lvpm.lock is written

  Scenario: an unreachable first source fails naming it, not falling through
    Given "second" lists "alpha" 1.0
    And "first" cannot be reached
    And the project depends on "alpha" "*"
    When I run "lvpm lock"
    Then the command fails naming "first"
    And no lvpm.lock is written

  Scenario: reordering [sources] makes the lock stale
    Given "first" lists "alpha" 1.0
    And "second" lists "alpha" 2.0
    And the project depends on "alpha" "*" and lvpm.lock is current
    When I swap "first" and "second" in [sources]
    And I run "lvpm lock --check"
    Then the exit code is non-zero
    And stderr is one line naming sources changed

  Scenario: a package in a local folder wins over a hosted source
    Given the local folder holds "alpha" 1.0
    And "first" lists "alpha" 2.0
    And the project depends on "alpha" "*"
    When I run "lvpm lock"
    Then the command succeeds
    And lvpm.lock takes "alpha" 1.0 from "local"

  Scenario: lvpm add writes a constraint from the first source's newest version
    Given "first" lists "alpha" 1.0
    And "second" lists "alpha" 2.0
    When I run "lvpm add alpha --prefix dest"
    Then the command succeeds
    And lvpm.toml depends on "alpha" ">=1.0"

  Scenario Outline: <command> shows only the first source's versions
    Given "first" lists "alpha" 1.0
    And "second" lists "alpha" 2.0
    When I run "<command>"
    Then the command succeeds
    And the output shows version 1.0 and not 2.0

    Examples:
      | command           |
      | lvpm info alpha   |
      | lvpm search alpha |
