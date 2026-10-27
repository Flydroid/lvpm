Feature: Destinations, venvs and launching LabVIEW

  Background:
    Given detected LabVIEW installations 2024, 2025 and 2026, 64-bit, at scratch folders

  Scenario: switch between the venv and a LabVIEW target
    Given the TUI runs in a project with a venv
    When "t" is pressed and LabVIEW 2026 is chosen
    Then the header reads "LabVIEW 2026 64-bit" and the statuses are relative to it
    When "t" is pressed and the venv is chosen
    Then the header names the venv again

  Scenario: a project without a venv offers to create one
    Given a project whose lvpm.toml asks for LabVIEW 2025 and has no venv
    When the TUI starts in it
    Then it shows the project and the footer offers "v create venv"
    And the LabVIEW choices are 2025 and 2026

  Scenario: create a venv
    Given a project whose lvpm.toml asks for LabVIEW 2025 and has no venv
    When "create venv" is chosen with LabVIEW 2025
    Then the venv exists and the header names it

  Scenario: remove the venv after a confirmation
    Given the destination is a project venv
    When "remove venv" is chosen
    Then a confirmation asks for "y"
    When Esc is pressed
    Then the venv still exists
    When "remove venv" is chosen and "y" is pressed
    Then the venv no longer exists and the TUI offers "create venv"

  Scenario: no eligible LabVIEW
    Given a project whose lvpm.toml asks for LabVIEW 2027 and has no venv
    When the TUI starts in it
    Then "create venv" is shown disabled, saying no LabVIEW 2027 or newer is installed

  Scenario: launch without a project file
    Given the destination is a project venv and the project has no .lvproj
    When "l" is pressed
    Then a launch of LabVIEW 2025 on the venv with no project file is requested
    And the footer says LabVIEW is launching

  Scenario: launch with a project file
    Given the destination is a project venv and the project has "app.lvproj"
    When "l" is pressed and "app.lvproj" is chosen
    Then a launch of LabVIEW 2025 on the venv opening app.lvproj is requested

  @labview
  Scenario: launch LabVIEW on the venv with a project file
    Given the destination is a project venv and the project has "app.lvproj"
    When "launch" is chosen with "app.lvproj"
    Then LabVIEW starts on the venv and opens app.lvproj
