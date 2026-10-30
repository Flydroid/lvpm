Feature: the .lvpkg format (S7a)
  `lvpm build` writes a .lvpkg. The keys ADR 6 removed fail with a message
  that says where each one went.

  Background:
    Given a project "demo_lib" version "1.2.3" with labview ">=2016"
    And [distribution] top-level-library "Lib.lvlib", vendor "Vendor", display-name "Demo Lib"
    And library "Lib.lvlib" holding "A.vi"

  Scenario Outline: a key ADR 6 removed fails, says what to do, and writes nothing
    Given the manifest <change>
    When lvpm build runs
    Then it fails with the "<case>" message, and dist is empty

    Examples:
      | change                                          | case               |
      | sets formats = ["vip"]                          | formats            |
      | sets formats = ["lvpkg"]                        | formats            |
      | has [distribution.vip] vendor = "Vendor"        | [distribution.vip] |
      | has an empty [distribution.vip] table           | [distribution.vip] |
      | has [distribution.vip.dependency-names] a = "b" | [distribution.vip] |

  Scenario: every key in [distribution.vip] is reported at once
    Given the manifest has [distribution.vip] with vendor, display-name, name and icon
    When lvpm build runs
    Then the message's "Found:" lists vendor, display-name, name and icon

  Scenario: a .lvpkg in a package folder installs
    Given a project depending on demo_lib, whose package folder (sources.local) holds the built "demo_lib-1.2.3.lvpkg"
    When lvpm install runs
    Then it succeeds, the lock records demo_lib 1.2.3 with the archive's SHA-256, and the venv holds the members below "vi.lib/Vendor/Demo Lib"

  Scenario: an imported .lvpkg installs
    Given "demo_lib-1.2.3.lvpkg" imported with lvpm cache add
    When a project depending on demo_lib runs lvpm install
    Then it succeeds and the venv holds the members

  Scenario: a .lvpkg from a hosted feed installs
    Given a hosted feed whose index.vipr names "demo_lib-1.2.3.lvpkg"
    When a project depending on demo_lib runs lvpm install
    Then it succeeds and the venv holds the members

  Scenario: a .vip and a .lvpkg in one package folder both resolve
    Given a package folder holding the built "demo_lib-1.2.3.lvpkg" and "other_lib-1.0.0.1.vip"
    When a project depending on both runs lvpm install
    Then it succeeds and the venv holds both packages' files

  Scenario: a .lvpkg renamed to .vip still installs
    Given a package folder holding the built package renamed "demo_lib-1.2.3.vip"
    When lvpm install runs
    Then it succeeds and the venv holds the members

  Scenario: a tampered .lvpkg from a hosted feed is refused before the venv is touched
    Given a hosted feed whose "demo_lib-1.2.3.lvpkg" has "vi.lib/Vendor/Demo Lib/A.vi" changed after build
    When a project depending on demo_lib runs lvpm install
    Then it fails with the "entry changed" message naming "vi.lib/Vendor/Demo Lib/A.vi" and "install demo_lib 1.2.3", and the venv is unchanged

  Scenario: cache add refuses a tampered .lvpkg
    Given "demo_lib-1.2.3.lvpkg" with "vi.lib/Vendor/Demo Lib/A.vi" changed after build
    When lvpm cache add runs on it
    Then it fails with the "entry changed" message naming "demo_lib-1.2.3.lvpkg" and the entry, and the cache is unchanged

  Scenario: a tampered .lvpkg in a package folder is ignored with a warning
    Given a package folder holding a good "other_lib-1.0.0.lvpkg" and a tampered "demo_lib-1.2.3.lvpkg"
    When a project depending on demo_lib and other_lib runs lvpm install
    Then stderr warns "! ignoring" the tampered file, naming "vi.lib/Vendor/Demo Lib/A.vi", the install fails because demo_lib is not found, and the venv is unchanged

  Scenario: cache add names the formats it takes
    When lvpm cache add runs on "x.zip"
    Then it fails, and the message names .vip, .ogp and .lvpkg
