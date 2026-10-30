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
    Then it fails with "--labview-version <value> is not a LabVIEW year"
    And the output says "nothing was written"
    And the folder has no lvpm.toml, no .git and no .lv-venv

    Examples:
      | value |
      | abc   |
      | 26.3  |

  Scenario: init makes a git repository and says how to add a remote
    Given an empty folder with no .git in it or above it
    When I run "lvpm init --labview-version 2023" in it
    Then the folder is a git repository in which "git rev-parse HEAD" fails
    And the output has the line "  add a remote:  git remote add origin <url>"
    And the output has the line "  first push:    git add -A && git commit -m "lvpm init" && git push -u origin HEAD"

  Scenario: inside a git repository, git is left alone
    Given a git repository with an empty subfolder "app"
    When I run "lvpm init --labview-version 2023" in "app"
    Then it succeeds
    And "app" has no .git of its own
    And the output says "already inside a git repository at" and the repository's folder

  # --labview-version 2023 always skips the venv: either 2023 is not
  # installed, or it is before 2024 Q1. Only the reason text varies.
  Scenario: a venv that cannot be made is a hint, not a failure
    Given an empty folder "App"
    When I run "lvpm init --labview-version 2023" in it
    Then it succeeds
    And the folder has no .lv-venv
    And the output has a line starting "no venv: " with a reason after it
    And the output has a line starting "  later: lvpm venv create"

  # The working directory is the project, so the venv is where the sandbox
  # looks for one and its cleanup finds a LabVIEW started on it.
  @labview @interactive-labview
  Scenario: the venv is bound and add installs into it
    Given a folder feed with the pilot packages
    When I run "lvpm init --labview-version 2026" in the working directory
    Then it succeeds
    And the folder has a venv bound to LabVIEW 2026
    When I run "lvpm add labview_open_source_project_lib_labview_open_source_palette" in the working directory
    Then it succeeds
    And the venv holds labview_open_source_project_lib_labview_open_source_palette in its add-on folder and no staging folder

  @labview @interactive-labview
  Scenario: labview is the newest installed
    Given the LabVIEW installations "lvpm targets" lists
    When I run "lvpm init" in an empty folder
    Then lvpm.toml says labview ">=" the newest listed year

  Scenario Outline: each .vipc package flag decides what init writes
    Given a VIPM configuration "p.vipc" for LabVIEW "13.0" with the package "demo_lib-1.5.0.15" marked dependency <dependency> and pinned <pinned>
    When I run "lvpm init --from p.vipc --labview-version 2023" in its folder
    Then it succeeds
    And lvpm.toml [dependencies] <outcome>

    Examples:
      | dependency | pinned | outcome                     |
      | false      | true   | has demo_lib = "1.5.0.15"   |
      | false      | false  | has demo_lib = ">=1.5.0.15" |
      | true       | false  | is empty                    |
      | true       | true   | is empty                    |

  Scenario: init reports what it added and skipped
    Given a VIPM configuration "p.vipc" with 2 chosen packages and 1 marked as a dependency
    When I run "lvpm init --from p.vipc --labview-version 2023" in its folder
    Then the output has the line "added 2 dependencies from p.vipc"
    And the output has a line "  <name> <constraint>" for each chosen package
    And the output has the line "  skipped 1 package VIPM marked as a dependency of another"

  Scenario: one package added, none skipped
    Given a VIPM configuration "p.vipc" with 1 chosen package and none marked as a dependency
    When I run "lvpm init --from p.vipc --labview-version 2023" in its folder
    Then the output has the line "added 1 dependency from p.vipc"
    And the output does not say "skipped"

  Scenario: a .vipc with no packages
    Given a VIPM configuration "p.vipc" with no packages
    When I run "lvpm init --from p.vipc --labview-version 2023" in its folder
    Then it succeeds
    And lvpm.toml [dependencies] is empty
    And the output has the line "added 0 dependencies from p.vipc"

  Scenario: the .vipc's LabVIEW is reported, not used
    Given a VIPM configuration "p.vipc" for LabVIEW "13.0"
    When I run "lvpm init --from p.vipc --labview-version 2023" in its folder
    Then lvpm.toml says labview ">=2023"
    And the output has the line "  the .vipc targets LabVIEW 2013; lvpm.toml says >=2023"

  Scenario: no report when the .vipc targets the chosen LabVIEW
    Given a VIPM configuration "p.vipc" for LabVIEW "23.0"
    When I run "lvpm init --from p.vipc --labview-version 2023" in its folder
    Then the output does not say "the .vipc targets"

  Scenario: --from is relative to the working directory, not --project
    Given a VIPM configuration "p.vipc" in the working directory and an empty folder "App"
    When I run "lvpm init --project App --from p.vipc --labview-version 2023" in the working directory
    Then it succeeds and "App" has an lvpm.toml with the .vipc's packages

  # The pilot feed holds only serializer 1.2.1.16, so ">=1.2.1.16" resolves to it.
  Scenario: lock resolves what init added
    Given a folder feed with the pilot packages
    And a VIPM configuration "Serializer.vipc" choosing labview_open_source_lib_serializer 1.2.1.16, not pinned
    When I run "lvpm init --from Serializer.vipc --labview-version 2023" in its folder
    And I run "lvpm lock" in the same folder
    Then the folder's lvpm.lock holds the serializer 1.2.1.16 and its two dependencies

  Scenario Outline: a bad --from file writes nothing
    Given <file>
    When I run "lvpm init --from p.vipc --labview-version 2023" in its folder
    Then it fails with "<text>"
    And the output says "nothing was written"
    And the folder has no lvpm.toml, no .git and no .lv-venv

    Examples:
      | file                                            | text                            |
      | no file "p.vipc"                                | p.vipc: cannot be read          |
      | a file "p.vipc" holding "hello"                 | p.vipc: not a zip file          |
      | a zip "p.vipc" with only "readme.txt"           | p.vipc: config.xml is missing   |
      | a zip "p.vipc" whose config.xml has no <Target> | p.vipc: config.xml: no <Target> |

  Scenario Outline: a .dragon file is refused
    Given <file>
    When I run "lvpm init --from <name>" in its folder
    Then it fails with ".dragon files are not supported and not planned"
    And the output says "export a .vipc from VIPM instead"
    And the folder has no lvpm.toml, no .git and no .lv-venv

    Examples:
      | file               | name     |
      | a file "p.dragon"  | p.dragon |
      | a file "P.DRAGON"  | P.DRAGON |
      | no file "p.dragon" | p.dragon |

  # No real .vipc with an included package could be made (it needs the VIPM
  # GUI), so these three put the archive where the design guesses it is:
  # <name>-<version>.vip at any depth.
  # `lvpm cache list` does not exist yet, so the cache is read through
  # `lvpm search --all`, which lists what `lvpm cache add` imported.
  Scenario: an embedded package is imported into the cache (synthetic layout)
    Given a VIPM configuration "p.vipc" that embeds labview_open_source_project_lib_labview_open_source_palette 1.0.2.7 as "Packages/labview_open_source_project_lib_labview_open_source_palette-1.0.2.7.vip"
    And no folder feed
    When I run "lvpm init --from p.vipc --labview-version 2023" in its folder
    Then the output has the line "  imported labview_open_source_project_lib_labview_open_source_palette 1.0.2.7 from p.vipc"
    And "lvpm search --all" lists labview_open_source_project_lib_labview_open_source_palette 1.0.2.7
    And "lvpm lock" succeeds in the folder

  Scenario: an Included package missing from the zip is a warning (synthetic layout)
    Given a VIPM configuration "p.vipc" marking demo_lib 1.0.0 included but holding no archive for it
    When I run "lvpm init --from p.vipc --labview-version 2023" in its folder
    Then it succeeds
    And lvpm.toml [dependencies] has demo_lib = ">=1.0.0"
    And the output says "warning: demo_lib 1.0.0 is marked Included but is not inside p.vipc"

  Scenario: a corrupt embedded package writes nothing (synthetic layout)
    Given a VIPM configuration "p.vipc" marking demo_lib 1.0.0 included, with a "demo_lib-1.0.0.vip" inside that is not a package
    When I run "lvpm init --from p.vipc --labview-version 2023" in its folder
    Then it fails with "p.vipc: embedded demo_lib-1.0.0.vip is not a package"
    And the folder has no lvpm.toml, no .git and no .lv-venv
    And the cache holds nothing

  @labview @interactive-labview
  Scenario: sync installs what init added
    Given a folder feed with the pilot packages
    And a VIPM configuration "Serializer.vipc" choosing labview_open_source_lib_serializer 1.2.1.16, not pinned
    When I run "lvpm init --from Serializer.vipc --labview-version 2026" in its folder
    And I run "lvpm sync" in the same folder
    Then it succeeds
    And the venv holds labview_open_source_lib_serializer in its add-on folder and no staging folder
    And the venv holds labview_open_source_lib_data_manipulation in its add-on folder and no staging folder
    And the venv holds labview_open_source_project_lib_labview_open_source_palette in its add-on folder and no staging folder

  # Built on the same guessed layout as the three above.
  @labview @interactive-labview
  Scenario: sync installs an embedded package with no feed
    Given a VIPM configuration "p.vipc" that embeds labview_open_source_project_lib_labview_open_source_palette 1.0.2.7
    And no folder feed
    When I run "lvpm init --from p.vipc --labview-version 2026" in its folder
    And I run "lvpm sync" in the same folder
    Then it succeeds
    And the venv holds labview_open_source_project_lib_labview_open_source_palette in its add-on folder and no staging folder
