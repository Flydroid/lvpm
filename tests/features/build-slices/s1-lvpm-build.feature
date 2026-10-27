Feature: lvpm build writes a .vip of a library's members
  `lvpm build` packages the library lvpm.toml names: the members its
  .lvlib lists, under one install path, with a spec lvpm reads back.

  Background:
    Given a project "demo_lib" version "1.2.3" with labview ">=2016"
    And [distribution] top-level-library "Lib.lvlib", vendor "Vendor", display-name "Demo Lib"
    And library "Lib.lvlib" holding "A.vi" and class "C/C.lvclass", which holds "C/M.vi"
    And "Stray.vi" next to the library, not a member

  Scenario: the package is written
    When lvpm build runs
    Then it succeeds and dist holds "demo_lib-1.2.3.0.vip"

  Scenario: build needs no LabVIEW
    Given no LabVIEW installation is visible to lvpm
    When lvpm build runs
    Then it succeeds

  Scenario: the built spec parses back
    When lvpm build runs
    Then the package's spec reads as demo_lib 1.2.3.0 for LabVIEW>=16.0, with one file group targeting "<application>"

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
    Given [distribution.vip] install-dir = "vi.lib/Other/Place"
    When lvpm build runs
    Then the file group holds the members under "vi.lib/Other/Place"

  Scenario: building twice gives the same bytes
    When lvpm build runs, every member's modification time is set to a different value, dist is emptied, and lvpm build runs again
    Then both packages are byte-identical

  Scenario: rebuilding replaces the package
    Given dist holds an older "demo_lib-1.2.3.0.vip" and "notes.txt"
    When lvpm build runs
    Then "demo_lib-1.2.3.0.vip" holds the current members and "notes.txt" is unchanged

  Scenario Outline: version is written as four parts
    Given the project version "<in>"
    When lvpm build runs
    Then dist holds "demo_lib-<out>.vip" and its spec Version is "<out>"

    Examples:
      | in      | out     |
      | 2.1.0   | 2.1.0.0 |
      | 2.1.0.5 | 2.1.0.5 |

  Scenario: dependencies become Requires
    Given dependencies a = "1.0", b = ">=2.0", c = "*" and "@hexagon/lib-a" = "3.0"
    And dependency-names maps "@hexagon/lib-a" to "hexagon_lib_a"
    When lvpm build runs
    Then the spec's Requires is "a>=1.0,b>=2.0,c,hexagon_lib_a>=3.0"

  Scenario Outline: a manifest problem fails naming the key and writes nothing
    Given the manifest <change>
    When lvpm build runs
    Then it fails, the message names the key "<key>" exactly, and dist holds no .vip

    Examples:
      | change                                                 | key                               |
      | has no [distribution] table                            | distribution                      |
      | has no top-level-library                               | distribution.top-level-library    |
      | names top-level-library "Missing.lvlib"                | distribution.top-level-library    |
      | names top-level-library "../Out.lvlib"                 | distribution.top-level-library    |
      | names top-level-library "C:/Abs.lvlib"                 | distribution.top-level-library    |
      | has no vendor                                          | distribution.vip.vendor           |
      | has no display-name                                    | distribution.vip.display-name     |
      | sets kinds = ["ppl"]                                   | distribution.kinds                |
      | sets kinds = ["source", "ppl"]                         | distribution.kinds                |
      | sets formats = ["lvpkg"]                               | distribution.formats              |
      | sets [distribution] colour = "red"                     | distribution.colour               |
      | sets [distribution.vip] icon                           | distribution.vip.icon             |
      | sets version = "1.2"                                   | project.version                   |
      | sets version = "1.2.3.4.5"                             | project.version                   |
      | sets version = "1.a.3"                                 | project.version                   |
      | names the project "@scope/lib" with no vip name        | distribution.vip.name             |
      | depends on "@scope/dep" with no dependency-names entry | distribution.vip.dependency-names |

  Scenario Outline: a member problem fails naming the member and writes nothing
    Given <problem>
    When lvpm build runs
    Then it fails naming "<name>" and dist holds no .vip

    Examples:
      | problem                                                  | name          |
      | the library lists "Gone.vi", which is not on disk        | Gone.vi       |
      | class "C/C.lvclass" lists "C/Lost.vi", not on disk       | Lost.vi       |
      | the library lists "../../Outside.vi", above its folder   | Outside.vi    |
      | "Lib.lvlib" is not a readable library                    | Lib.lvlib     |

  Scenario: a failed build leaves the previous package
    Given dist holds a built "demo_lib-1.2.3.0.vip"
    And the manifest has no vendor
    When lvpm build runs
    Then it fails and that package is unchanged

  Scenario: the docs name lvpm build
    Then docs/usage.md has a command-table line for lvpm build and documents the [distribution] keys
    And README.md shows lvpm build
