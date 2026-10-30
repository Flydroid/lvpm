Feature: lvpm init starts a project (S10a)
  `lvpm init` writes an lvpm.toml named after the folder and never replaces one
  that is there. It reads no feed and starts no LabVIEW.

  # lvpm lock reads the index of the default source even with no dependencies.
  # The sandbox answers that from its loopback feed, and lock starts no LabVIEW.
  Scenario: init writes a manifest
    Given an empty folder "Serializer"
    When I run "lvpm init --labview-version 2023" in it
    Then it succeeds
    And lvpm.toml has project name "Serializer", version "0.1.0" and labview ">=2023"
    And lvpm.toml has an empty [dependencies] table
    And the output has the line "wrote <path of lvpm.toml>"
    And "lvpm lock" succeeds in the folder

  Scenario: --project names the folder
    Given an empty folder "Elsewhere"
    When I run "lvpm init --project Elsewhere --labview-version 2023" in the working directory
    Then "Elsewhere" has an lvpm.toml with project name "Elsewhere"
    And the working directory has no lvpm.toml

  Scenario: --project naming a missing folder writes nothing
    When I run "lvpm init --project Missing --labview-version 2023" in the working directory
    Then it fails with "Missing does not exist; nothing was written"
    And the working directory has no "Missing" folder and no lvpm.toml

  Scenario: an existing lvpm.toml is never overwritten
    Given a folder with an lvpm.toml
    When I run "lvpm init --labview-version 2023" in it
    Then it fails with "<path of lvpm.toml> already exists; lvpm init does not overwrite it"
    And the output says "lvpm add puts packages in it"
    And lvpm.toml is unchanged
    And the folder has no .git and no .lv-venv

  Scenario: a manifest in a parent folder does not block
    Given a folder "Parent" with an lvpm.toml and an empty subfolder "app"
    When I run "lvpm init --labview-version 2023" in "Parent/app"
    Then it succeeds
    And "Parent/app" has its own lvpm.toml with project name "app"
    And the parent's lvpm.toml is unchanged

  Scenario Outline: an unusable --labview-version writes nothing
    Given an empty folder "App"
    When I run "lvpm init --labview-version <value>" in it
    Then it fails with "--labview-version <value>"
    And the output says "nothing was written"
    And the folder has no lvpm.toml, no .git and no .lv-venv

    Examples:
      | value |
      | abc   |
      | 26.3  |

  @labview @interactive-labview
  Scenario: labview is the newest installed
    Given the LabVIEW installations "lvpm targets" lists
    When I run "lvpm init" in an empty folder
    Then lvpm.toml says labview ">=" the newest listed year
