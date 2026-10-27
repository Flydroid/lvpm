Feature: Every screen fits and every action has a key

  Scenario Outline: every screen renders whole at 80x24
    Given the TUI runs at 80x24
    When the <screen> screen is opened with its key
    Then no line is wider than 80 columns
    And every key hint of the footer is shown whole
    And the screen's borders are closed
    When the screen's leave key is pressed
    Then the previous screen is shown

    Examples:
      | screen      |
      | details     |
      | preview     |
      | progress    |
      | result      |
      | log         |
      | destination |
      | maintenance |
      | config      |
      | help        |

  Scenario: help lists every key
    When "?" is pressed
    Then the help lists every key of the key table, grouped by screen
    And no entry is marked "planned"

  Scenario: the README documents lvpm-tui
    When the README is read
    Then it says how to start lvpm-tui and lists its keys

  Scenario: colour is never the only signal
    Given NO_COLOR is set
    When the list shows an installed, an update and an incompatible package
    Then each row's status is readable as text
