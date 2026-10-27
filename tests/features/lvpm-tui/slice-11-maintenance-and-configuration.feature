Feature: Maintenance and configuration

  Scenario Outline: maintenance for one package or for all
    Given the TUI runs on a scratch destination with "alpha" installed
    When "m" is pressed and "<action>" is chosen for <scope>
    Then a preview names <scope>
    When Enter is pressed
    Then the progress view shows the <action> phase as skipped for a scratch destination
    And the result says done

    Examples:
      | action    | scope           |
      | relink    | "alpha"         |
      | run-hooks | all packages    |
      | refresh   | the destination |

  @labview
  Scenario: relink runs on a real LabVIEW
    Given the TUI runs on a LabVIEW target with "alpha" installed
    When "m" is pressed, "relink" is chosen for "alpha" and Enter is pressed
    Then the progress view shows relink done

  Scenario: the config view shows each value and where it comes from
    Given a sandboxed user config with "venv.dir" set
    And the environment sets LVPM_CONFIG_CACHE
    When "C" is pressed
    Then "venv.dir" shows its value with origin "file"
    And "cache" shows its value with origin "environment"
    And "sources.local" shows origin "default"

  Scenario: editing a setting writes the user config file
    When in the config view "sources.local" is selected, Enter is pressed, "C:\pkgs" is typed and Enter is pressed
    Then the user config file has sources.local = ["C:\pkgs"]
    And the view shows origin "file"

  Scenario: Esc cancels an edit
    When in the config view "venv.dir" is selected, Enter is pressed, "x" is typed and Esc is pressed
    Then the user config file is unchanged

  Scenario: an empty value clears the setting
    Given the user config sets "venv.dir"
    When "venv.dir" is edited to an empty value
    Then the user config file no longer sets "venv.dir" and the view shows origin "default"

  Scenario: an invalid value is refused and the edit stays open
    When in the config view "cache" is selected, Enter is pressed, "a;b" is typed and Enter is pressed
    Then the view shows core's error that cache expects one value
    And the edit field still holds "a;b"
    And the user config file is unchanged

  Scenario: a list setting takes several values
    When "sources.local" is edited to "C:\pkgs;D:\more"
    Then the user config file has sources.local = ["C:\pkgs", "D:\more"]

  Scenario: editing a setting the environment overrides warns
    Given the environment sets LVPM_CONFIG_CACHE
    When "cache" is edited in the config view
    Then a warning says the edit has no effect while LVPM_CONFIG_CACHE is set
