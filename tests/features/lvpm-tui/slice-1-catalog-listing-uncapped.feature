Feature: Every version of every package, uncapped

  Scenario: the catalog lists every version from every source
    Given a feed with "alpha" 1.0 and 2.0
    And a local folder with "beta" 0.1
    When "lvpm search --all" runs
    Then it lists "alpha" 2.0, "alpha" 1.0 and "beta" 0.1, in that order
    And each row shows its display name, requirements, minimum LabVIEW and source
    And the output matches the "search-all" recording

  Scenario: no cap on the number of rows
    Given a feed with 60 packages of 2 versions each
    When "lvpm search --all" runs
    Then it lists 120 rows and no "more" line

  Scenario Outline: a query narrows the catalog by name or display name, ignoring case
    Given a feed with "alpha" 1.0, shown as "Alpha Tools", and "beta" 0.1
    When "lvpm search --all <query>" runs
    Then it lists only "alpha" 1.0

    Examples:
      | query |
      | alp   |
      | ALP   |
      | tools |

  Scenario: an empty catalog says so and names the sources
    Given a feed and a local folder with no package
    When "lvpm search --all" runs
    Then it says that no package was found in that feed and that folder
    And it exits 0

  Scenario: one source cannot be read
    Given a feed with "alpha" 1.0 that does not answer and has never been cached
    And a local folder with "beta" 0.1
    When "lvpm search --all" runs
    Then it lists "beta" 0.1
    And stderr names the feed that could not be read
    And it exits 0

  Scenario: every source fails
    Given the only feed does not answer and has never been cached
    When "lvpm search --all" runs
    Then it fails with exit code 1 and stderr names the feed

  Scenario: a query that matches nothing
    Given a feed with "alpha" 1.0
    When "lvpm search --all zzz" runs
    Then it says no package matches "zzz"
    And it exits 0

  Scenario: search needs a query or --all
    When "lvpm search" runs
    Then it fails as a usage error with exit code 2

  Scenario: plain search is unchanged
    When the characterization rows "search-a" and "search-none" run
    Then they still match
