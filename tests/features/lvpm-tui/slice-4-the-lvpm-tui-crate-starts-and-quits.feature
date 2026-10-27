Feature: lvpm-tui is a workspace binary on lvpm-core

  Scenario: the crate depends on core only
    When the workspace's dependency graph is read
    Then "lvpm-tui" depends on "lvpm-core" and not on "lvpm"
    And "lvpm-core" depends on neither binary crate

  Scenario: both binaries report one version
    When "lvpm --version" and "lvpm-tui --version" run
    Then both print the workspace version

  Scenario: the TUI starts and quits
    Given the TUI runs at 80x24
    Then the header bar and the key footer are drawn with their borders
    When "q" is pressed
    Then the TUI's event loop ends without an error

  Scenario: a key release is not a second press
    Given the TUI runs at 80x24
    When "q" is released without a press
    Then the TUI is still running

  Scenario Outline: a terminal smaller than 80x24
    Given the TUI runs at <size>
    Then it shows only a message asking for at least 80x24
    When the terminal is resized to 80x24
    Then the header bar and the key footer are drawn

    Examples:
      | size  |
      | 79x24 |
      | 80x23 |
      | 70x20 |
