Feature: Browse the catalog

  Background:
    Given the TUI runs at 80x24
    And a source with "alpha" 1.0 and 2.0, "beta" 1.0, and "gamma" 1.0 that needs LabVIEW 2099
    And one detected LabVIEW, 2025 64-bit, at a scratch folder

  Scenario: inside a project with a venv the venv is the destination
    Given a project with a venv on LabVIEW 2025
    When the TUI starts in the project
    Then the header reads "venv: <project>/.venv · LabVIEW 2025 64-bit"

  Scenario: outside a project the detected LabVIEW is the destination
    When the TUI starts outside any project
    Then the header reads "LabVIEW 2025 64-bit"

  Scenario: outside a project with no LabVIEW and no prefix
    Given no LabVIEW is detected
    When the TUI starts outside any project
    Then the header says no destination is selected
    And the footer says install needs a LabVIEW or --prefix

  Scenario: every package has a status relative to the destination
    Given "alpha" 1.0 and "beta" 1.0 are installed in the destination
    When the list has loaded
    Then "alpha" is "update" with installed 1.0 and available 2.0
    And "beta" is "installed"
    And "gamma" is "needs LV 2099"
    And every other package is "not installed"

  Scenario: no update is offered when the newer version is incompatible
    Given "delta" 1.0 is installed and "delta" 2.0 needs LabVIEW 2099
    When the list has loaded
    Then "delta" is "installed"

  Scenario: the list moves with the keys
    Given a source with 60 packages
    When the list has loaded and Down is pressed 30 times
    Then the 31st package is selected and visible
    When End is pressed
    Then the last package is selected and visible
    When Home is pressed
    Then the first package is selected

  Scenario Outline: search narrows by name or display name
    When "/" is pressed and "<typed>" is typed
    Then only "alpha" is listed
    When Esc is pressed
    Then every package is listed again

    Examples:
      | typed |
      | alp   |
      | ALP   |

  Scenario: the filter shows only updates
    Given "alpha" 1.0 is installed
    When "f" is pressed until the filter reads "updates"
    Then only "alpha" is listed

  Scenario: a search that matches nothing
    When "/" is pressed and "zzz" is typed
    Then the list says that no package matches "zzz"

  Scenario: install is refused for an incompatible package
    When "gamma" is selected and "i" is pressed
    Then no plan preview opens
    And the footer says "gamma" needs LabVIEW 2099

  Scenario: the list is usable while the catalog loads
    Given the feed is paused
    When the TUI starts
    Then a loading line is shown
    And "q" quits

  Scenario: no source has any package
    Given no source has a package
    When the list has loaded
    Then the list says the catalog is empty and names the configured sources

  Scenario: every source fails
    Given the only feed does not answer and has never been cached
    When the TUI starts
    Then the screen shows core's error naming the feed
    And the footer offers "R reload feeds" and "q quit"
    When the feed answers again and "R" is pressed
    Then the list shows "alpha", "beta" and "gamma"

  Scenario: one source fails
    Given a second feed that does not answer and has never been cached
    When the list has loaded
    Then a banner names the second feed as not read
    And the packages of the first feed are listed
