Feature: lvpm build writes a .lvpkg of a library's members
  `lvpm build` packages the library lvpm.toml names: the members its
  .lvlib lists, under one install path, with a package.toml lvpm reads back.

  Background:
    Given a project "demo_lib" version "1.2.3" with labview ">=2016"
    And [distribution] top-level-library "Lib.lvlib", vendor "Vendor", display-name "Demo Lib"
    And library "Lib.lvlib" holding "A.vi" and class "C/C.lvclass", which holds "C/M.vi"
    And "Stray.vi" next to the library, not a member

  Scenario: the package is written
    When lvpm build runs
    Then it succeeds, prints the path of "dist/demo_lib-1.2.3.lvpkg", and dist holds no .vip

  Scenario: build needs no LabVIEW
    Given no LabVIEW installation is visible to lvpm
    When lvpm build runs
    Then it succeeds

  Scenario: the built package reads back
    When lvpm build runs
    Then the package's package.toml reads as demo_lib 1.2.3, labview ">=2016", kind "source", vendor "Vendor", display-name "Demo Lib", install-dir "vi.lib/Vendor/Demo Lib"

  Scenario: default install path
    When lvpm build runs
    Then the file group holds Lib.lvlib, A.vi, C/C.lvclass and C/M.vi under "vi.lib/Vendor/Demo Lib"

  Scenario: a stray file is left out
    When lvpm build runs
    Then the package holds no "Stray.vi"

  Scenario: members of a nested class are in
    When lvpm build runs
    Then the file group holds "C/C.lvclass" and "C/M.vi"

  Scenario: an auto-populating folder contributes its files
    Given the library has an auto-populating folder "Data" holding "d.txt" on disk
    When lvpm build runs
    Then the file group holds "Data/d.txt"

  Scenario: install-dir places the members
    Given [distribution] install-dir = "vi.lib/Other/Place"
    When lvpm build runs
    Then the file group holds the members under "vi.lib/Other/Place"

  Scenario: building twice gives the same bytes
    When lvpm build runs, every member's modification time is set to a different value, dist is emptied, and lvpm build runs again
    Then both packages are byte-identical

  Scenario: rebuilding replaces the package
    Given dist holds an older "demo_lib-1.2.3.lvpkg" and "notes.txt"
    When lvpm build runs
    Then "demo_lib-1.2.3.lvpkg" holds the current members and "notes.txt" is unchanged

  Scenario Outline: the version is written as lvpm.toml has it
    Given the project version "<v>"
    When lvpm build runs
    Then dist holds "demo_lib-<v>.lvpkg" and its package.toml version is "<v>"

    Examples:
      | v       |
      | 2.1.0   |
      | 2.1.0.5 |

  Scenario: dependencies are written as lvpm.toml has them
    Given dependencies a = "1.0", b = ">=2.0", c = "*" and "@hexagon/lib-a" = "3.0"
    When lvpm build runs
    Then package.toml's [dependencies] holds those four, as written, in that order

  Scenario: an old .vip in dist stays
    Given dist holds "demo_lib-1.2.3.0.vip"
    When lvpm build runs
    Then "demo_lib-1.2.3.0.vip" is unchanged

  Scenario Outline: a manifest problem fails naming the key and writes nothing
    Given the manifest <change>
    When lvpm build runs
    Then it fails, the message names the key "<key>" exactly, and dist holds no .lvpkg

    Examples:
      | change                                                 | key                               |
      | has no [distribution] table                            | distribution                      |
      | has no top-level-library                               | distribution.top-level-library    |
      | names top-level-library "Missing.lvlib"                | distribution.top-level-library    |
      | names top-level-library "../Out.lvlib"                 | distribution.top-level-library    |
      | names top-level-library "C:/Abs.lvlib"                 | distribution.top-level-library    |
      | has no vendor                                          | distribution.vendor               |
      | has no display-name                                    | distribution.display-name         |
      | sets install-dir = "../x"                              | distribution.install-dir          |
      | sets install-dir = "a/../x"                            | distribution.install-dir          |
      | sets install-dir = "C:/x"                              | distribution.install-dir          |
      | sets install-dir = "/x"                                | distribution.install-dir          |
      | sets install-dir = "a\\b"                              | distribution.install-dir          |
      | sets kinds = ["ppl"]                                   | distribution.kinds                |
      | sets kinds = ["source", "ppl"]                         | distribution.kinds                |
      | sets [distribution] colour = "red"                     | distribution.colour               |
      | sets version = "1.2"                                   | project.version                   |
      | sets version = "1.2.3.4.5"                             | project.version                   |
      | sets version = "1.a.3"                                 | project.version                   |

  Scenario: a scoped project name is refused
    Given the manifest names the project "@scope/lib"
    When lvpm build runs
    Then it fails with the message `project.name = "@scope/lib": scoped package names are not supported yet; use an unscoped name`, and dist holds no .lvpkg

  Scenario Outline: a member problem fails naming the member and writes nothing
    Given <problem>
    When lvpm build runs
    Then it fails naming "<name>" and dist holds no .lvpkg

    Examples:
      | problem                                                  | name          |
      | the library lists "Gone.vi", which is not on disk        | Gone.vi       |
      | class "C/C.lvclass" lists "C/Lost.vi", not on disk       | Lost.vi       |
      | the library lists "../../Outside.vi", above its folder   | Outside.vi    |
      | "Lib.lvlib" is not a readable library                    | Lib.lvlib     |

  Scenario: a failed build leaves the previous package
    Given dist holds a built "demo_lib-1.2.3.lvpkg"
    And the manifest has no vendor
    When lvpm build runs
    Then it fails and that package is unchanged

  Scenario: the docs name lvpm build
    Then docs/usage.md has a command-table line for lvpm build, names the .lvpkg output, and documents vendor, display-name and install-dir under [distribution]
    And docs/usage.md mentions [distribution.vip] only in its upgrade note
    And README.md shows lvpm build writing a .lvpkg
    And ADR 1's example has no formats and no [distribution.vip]

  Scenario: nothing refers to the deleted writer
    Then no tracked file outside the history names the .vip writer, the VIPM probe or the S1b feature
