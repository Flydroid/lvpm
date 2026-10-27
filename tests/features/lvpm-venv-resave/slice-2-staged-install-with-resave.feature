Feature: A venv install resaves each package before it lands
  A package bound for a venv is staged next to it, resaved in the venv's
  own LabVIEW, and only then moved into its add-on folder. The scenarios
  without a LabVIEW are unit tests in lvpm-core (ops/resave.rs); this file
  holds the ones that need the real LabVIEW 2026.

  @labview
  Scenario: the VI fixture lands through staging
    Given a venv for LabVIEW 2026 and the VI fixture package
    And LV_RTE_HEADLESS is set
    When I run "lvpm install lvpm_vi_fixture --no-relink --labview-version 2026"
    Then the exit code is 0
    And the output has a resave line for lvpm_vi_fixture
    And the venv holds lvpm_vi_fixture in its add-on folder and no staging folder
