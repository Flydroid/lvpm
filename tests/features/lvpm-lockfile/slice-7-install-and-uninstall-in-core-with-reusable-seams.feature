Feature: Install and uninstall run in core through plan, apply and finish

  Scenario Outline: install and uninstall output is unchanged
    When the characterization row "<name>" runs
    Then it still matches

    Examples:
      | name              |
      | install-dry-run   |
      | install-beta      |
      | install-hooked    |
      | install-missing   |
      | uninstall-beta    |
      | uninstall-missing |

  @core
  Scenario: a scratch install emits its phases in order
    Given a project using only the loopback feed
    When core installs "beta" into a scratch destination with a recording sink
    Then the phases appear in this order: resolve, download, unpack
    And relink, hooks and refresh are each reported as skipped with reason "scratch destination"

  @core
  Scenario: apply accepts entries that did not come from greedy resolution
    Given an install plan holding "alpha" 1.0 built by hand
    When core applies and finishes it into a scratch destination
    Then "alpha" 1.0 is installed and its install manifest records the archive's SHA-256

  @labview
  Scenario: venv install output is unchanged
    When the characterization row "venv-install-beta" runs
    Then it still matches
