Feature: lvpm publish copies the current package into a folder feed
  `lvpm publish --to <dir>` copies dist/<name>-<version>.vip, as lvpm build
  named it, into <dir>. It never replaces a file there.

  Background:
    Given a project "demo_lib" version "1.2.3" with labview ">=2016"
    And [distribution] top-level-library "Lib.lvlib", vendor "Vendor", display-name "Demo Lib"
    And library "Lib.lvlib" holding "A.vi" and class "C/C.lvclass", which holds "C/M.vi"
    And an empty folder "feed"

  Scenario: the package is published
    Given lvpm build has run
    When lvpm publish --to "feed" runs
    Then it succeeds and prints the path of "feed/demo_lib-1.2.3.0.vip"
    And "feed" holds exactly "demo_lib-1.2.3.0.vip", byte-identical to "dist/demo_lib-1.2.3.0.vip"

  Scenario: publish copies what was built, not what is on disk now
    Given lvpm build has run
    And "A.vi" is changed after the build
    When lvpm publish --to "feed" runs
    Then "feed" holds exactly "demo_lib-1.2.3.0.vip", byte-identical to the package as built

  Scenario Outline: an existing file is never replaced
    Given lvpm build has run
    And "feed" already holds "demo_lib-1.2.3.0.vip" with <content>
    When lvpm publish --to "feed" runs
    Then it fails naming "feed/demo_lib-1.2.3.0.vip"
    And "feed" holds exactly "demo_lib-1.2.3.0.vip", with its earlier bytes

    Examples:
      | content                     |
      | the same bytes as the build |
      | the text "older package"    |

  Scenario: publishing twice fails the second time
    Given lvpm build has run
    And lvpm publish --to "feed" has run
    When lvpm publish --to "feed" runs
    Then it fails naming "feed/demo_lib-1.2.3.0.vip"
    And "feed" holds exactly "demo_lib-1.2.3.0.vip", with its earlier bytes

  Scenario: publish does not build
    When lvpm publish --to "feed" runs
    Then it fails naming "dist/demo_lib-1.2.3.0.vip" and saying to run lvpm build
    And dist holds no .vip
    And "feed" is empty

  Scenario Outline: the target is not a folder
    Given lvpm build has run
    And <target>
    When lvpm publish --to "<path>" runs
    Then it fails naming "<path>"
    And "<path>" is as it was, and dist holds only "demo_lib-1.2.3.0.vip"

    Examples:
      | target                             | path    |
      | nothing named "missing" exists     | missing |
      | a file "plain" holding "not a dir" | plain   |

  Scenario: no lvpm.toml
    Given the work directory has no lvpm.toml
    When lvpm publish --to "feed" runs
    Then it fails naming "lvpm.toml"
    And "feed" is empty

  # The manifest is checked before the package is looked for, so these fail
  # on the manifest even though a package was built.
  Scenario Outline: an invalid manifest fails naming the key
    Given lvpm build has run
    And the manifest <change>
    When lvpm publish --to "feed" runs
    Then it fails, the message names the key "<key>" exactly, and "feed" is empty

    Examples:
      | change                      | key                     |
      | has no [distribution] table | distribution            |
      | has no vendor               | distribution.vip.vendor |

  Scenario: an older version's package stays behind
    Given lvpm build has run for version "1.2.2"
    And lvpm build has run for version "1.2.3"
    And dist holds "demo_lib-1.2.2.0.vip" and "demo_lib-1.2.3.0.vip"
    When lvpm publish --to "feed" runs
    Then "feed" holds exactly "demo_lib-1.2.3.0.vip", byte-identical to "dist/demo_lib-1.2.3.0.vip"

  Scenario: only an older version's package was built
    Given lvpm build has run for version "1.2.2"
    And the version is changed to "1.2.3" without a build
    When lvpm publish --to "feed" runs
    Then it fails naming "dist/demo_lib-1.2.3.0.vip" and saying to run lvpm build
    And "feed" is empty

  Scenario: --to is required
    Given lvpm build has run
    When lvpm publish runs with no --to
    Then it fails and the usage names --to
    And "feed" is empty

  Scenario: --to resolves from where the command ran
    Given lvpm build has run
    And an empty folder "elsewhere/feed"
    When lvpm publish --project <the project> --to "feed" runs from "elsewhere"
    Then "elsewhere/feed" holds exactly "demo_lib-1.2.3.0.vip", byte-identical to "dist/demo_lib-1.2.3.0.vip"
    And "feed" is empty

  Scenario: a project locks from the published folder
    Given lvpm build has run
    And lvpm publish --to "feed" has run
    And a second project depending on "demo_lib" ">=1.2", with "feed" as its only local source
    When lvpm lock runs in the second project
    Then it succeeds and lvpm.lock holds "demo_lib" version "1.2.3.0" from a local source

  Scenario: the docs name lvpm publish
    Then docs/usage.md has a command-table line for lvpm publish
    And docs/usage.md's section "Publishing a package" follows "Building a package" and holds "--to" and "never replaced"
    And README.md shows lvpm publish
