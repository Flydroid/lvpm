@core
Feature: lvpm edits one manifest entry and keeps the rest of the file

  Background:
    Given a manifest with a header comment, [project], a [sources] table with a trailing comment, and [dependencies] "alpha" = "1.0", "beta" = ">=1.0"

  Scenario: adding an entry adds exactly one line
    When "gamma" = ">=1.0" is written to [dependencies]
    Then the file differs from before by exactly one added line in [dependencies]

  Scenario: replacing an entry changes only its value
    When "alpha" is set to "2.0"
    Then only the value of "alpha" differs from before

  Scenario: removing an entry removes only its line
    When "alpha" is removed
    Then the file differs from before by exactly the "alpha" line

  Scenario: removing the last entry keeps the table header
    Given [dependencies] holds only "alpha"
    When "alpha" is removed
    Then [dependencies] is present and empty and nothing else changed

  Scenario: writing to a missing table creates it at the end
    Given the manifest has no [dev-dependencies] table
    When "caraya" = ">=1.0" is written as a dev dependency
    Then a [dev-dependencies] table with that entry is appended and nothing else changed

  Scenario: an entry with a same-line comment keeps its comment
    Given "beta" = ">=1.0"  # pinned for DQMH
    When "beta" is set to ">=2.0"
    Then that line reads "beta" = ">=2.0"  # pinned for DQMH

  Scenario: an unchanged write does not touch the file
    When "alpha" is set to "1.0"
    Then the file's bytes and modification time are unchanged

  Scenario: CRLF line endings are kept
    Given the manifest uses CRLF line endings
    When "gamma" = ">=1.0" is written to [dependencies]
    Then every line of the file still ends in CRLF

  Scenario: invalid TOML is not edited
    Given the manifest is not valid TOML
    When "gamma" = ">=1.0" is written to [dependencies]
    Then the edit fails naming the line that cannot be parsed and the file is unchanged

  Scenario: dev dependencies are read with the same syntax
    Given [dev-dependencies] "caraya" = "*"
    When the manifest is read
    Then "caraya" is a dev dependency with any version

  Scenario: an unsupported constraint in [dev-dependencies] is rejected
    Given [dev-dependencies] "caraya" = "~1.0"
    When the manifest is read
    Then reading fails naming "caraya" and the unsupported constraint "~1.0"
