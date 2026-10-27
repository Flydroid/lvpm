Feature: Uninstall and batches in global mode

  Background:
    Given the TUI runs at 80x24 on a scratch destination
    And "alpha" 1.0, which requires "beta" 1.0, and "beta" 1.0 are installed

  Scenario: uninstall is refused while another package requires it
    When "beta" is selected and "x" is pressed
    Then the screen says "beta" is required by "alpha"
    And no uninstall runs

  Scenario: an installed package no catalog knows makes the check incomplete
    Given "delta" 1.0 is installed and no source has it
    When "alpha" is selected and "x" is pressed
    Then the preview says the dependents check is incomplete because of "delta"
    When "y" is pressed
    Then "alpha" is no longer installed

  Scenario: a known dependent still refuses when another package is unknown
    Given "delta" 1.0 is installed and no source has it
    When "beta" is selected and "x" is pressed
    Then the screen says "beta" is required by "alpha"
    And no uninstall runs

  Scenario: uninstall of a package nothing requires
    When "alpha" is selected and "x" is pressed
    Then the preview lists the files of "alpha" to remove, from a dry run
    And nothing is removed yet
    When "y" is pressed
    Then "alpha" is no longer installed

  Scenario: Esc on the uninstall preview changes nothing
    When "alpha" is selected, "x" is pressed and Esc is pressed
    Then "alpha" is still installed

  Scenario: a failed uninstall is shown
    Given a file of "alpha" in the destination cannot be removed
    When "alpha" is selected, "x" is pressed and "y" is pressed
    Then the result shows core's error naming that file
    And the list shows "alpha" as core reports it after the failure

  Scenario: a multi-select runs one operation per package after one preview
    Given "epsilon" 1.0 and "zeta" 1.0 are in the catalog
    When "epsilon" and "zeta" are marked with Space
    Then both rows show "*" and the count line says 2 marked
    When "i" is pressed
    Then one preview lists both
    When Enter is pressed
    Then the result reports "epsilon" and "zeta" separately, in that order
    And no row is marked

  Scenario: an incompatible marked package is refused in the preview
    Given "gamma" 1.0 needs LabVIEW 2099
    When "epsilon" and "gamma" are marked and "i" is pressed
    Then the preview lists "epsilon" and says "gamma" is left out because it needs LabVIEW 2099

  Scenario: upgrade all upgrades every package with an update
    Given the catalog has "alpha" 2.0 and "beta" 2.0
    When "U" is pressed and Enter is pressed
    Then "alpha" 2.0 and "beta" 2.0 are installed

  Scenario: cancel ends the batch
    Given "epsilon", "zeta" and "eta" 1.0 are in the catalog
    When a batch installing all three is paused at the download of "zeta"
    And "c" is pressed
    Then "epsilon" is done, "zeta" is cancelled and "eta" is skipped

  Scenario: a failure ends the batch
    Given "epsilon", "zeta" and "eta" 1.0 are in the catalog and the archive of "zeta" is corrupt
    When a batch installing all three runs
    Then "epsilon" is done, "zeta" failed with core's error and "eta" is skipped

  Scenario: upgrade all with nothing to upgrade
    Given no installed package has an update
    When "U" is pressed
    Then the footer says everything is up to date
