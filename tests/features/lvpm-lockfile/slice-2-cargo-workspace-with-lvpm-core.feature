Feature: lvpm is built from a core library and a CLI on top of it

  Scenario: every pre-split unit test still exists and passes
    Given the list of unit tests recorded before the split
    When the workspace's unit tests are listed and run
    Then every recorded test name is present under its new crate path
    And all of them pass

  Scenario: core does not depend on the CLI
    When the workspace dependency graph is inspected
    Then "lvpm-core" depends on no binary crate of the workspace

  Scenario: both crates report one version
    When the version of "lvpm" and "lvpm-core" is read
    Then they are equal
