Feature: Package details

  Background:
    Given a feed with "alpha" 1.0 and 2.0
    And the 2.0 spec has a description, summary, vendor, packager, license, URL and release notes
    And "alpha" 2.0 requires "beta" 0.1

  Scenario: info shows the spec fields, the versions, the requirements and the files
    When "lvpm info alpha" runs
    Then it shows versions 2.0 and 1.0, each with its source
    And it shows the description, summary, vendor, packager, license, URL and release notes of 2.0
    And it shows that 2.0 requires "beta" >= 0.1
    And it lists the file groups of 2.0 with their target directories and files
    And the output matches the "info-alpha" recording

  # The binary would find this machine's LabVIEW; core is handed none.
  @core
  Scenario: info needs no LabVIEW
    When core reads the details of "alpha" with no LabVIEW installed
    Then the details are those of 2.0

  Scenario: a chosen version
    When "lvpm info alpha@1.0" runs
    Then the spec fields and files shown are those of 1.0

  Scenario: fields the spec does not have are left out
    Given "alpha" 1.0 has no [Description] section
    When "lvpm info alpha@1.0" runs
    Then no description, summary, vendor, packager, license, URL or release-notes line is shown

  Scenario: a fetch says so on stderr
    When "lvpm info alpha" runs with an empty cache
    Then stderr says it is fetching "alpha" 2.0
    And stdout holds only the details

  Scenario: the second request does not download again
    Given "lvpm info alpha" has run once
    When "lvpm info alpha" runs again
    Then the feed serves no archive request

  Scenario: another version is downloaded once
    Given "lvpm info alpha" has run once
    When "lvpm info alpha@1.0" runs
    Then the feed serves one archive request, for 1.0

  Scenario: info and install share the cache
    Given "lvpm info alpha" has run once
    When "lvpm install alpha --prefix dest" runs
    Then the feed serves no archive request for "alpha" 2.0

  Scenario: an unknown package fails and names the sources searched
    When "lvpm info nothing" runs
    Then it fails with exit code 1
    And the error names every configured feed and local folder

  Scenario: an unknown version fails and lists the known ones
    When "lvpm info alpha@9.9" runs
    Then it fails with exit code 1 and lists the versions 2.0 and 1.0

  Scenario: an archive that cannot be fetched fails and names the package and the feed
    Given the archive request of "alpha" 2.0 fails
    When "lvpm info alpha" runs with an empty cache
    Then it fails with exit code 1 and the error names "alpha" 2.0 and the feed

  Scenario: a corrupt archive fails and names the package
    Given the archive of "alpha" 2.0 is corrupt
    When "lvpm info alpha" runs
    Then it fails with exit code 1 and the error names "alpha" 2.0
