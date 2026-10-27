Feature: Package details and reloading the feeds

  Background:
    Given the TUI runs at 80x24 with a feed holding "alpha" 1.0 and 2.0
    And the 2.0 spec has a description, vendor and license

  Scenario: details show the versions, requirements and spec fields
    When "alpha" is selected and Enter is pressed
    Then the detail view lists 2.0 and 1.0
    And once loaded, it shows the description, vendor and license of 2.0 and its requirements

  Scenario: the detail view accepts input while it loads
    Given the feed holds archive downloads
    When the detail view of "alpha" opens
    Then it shows a loading state
    And Down selects 1.0

  Scenario: picking a version shows that version's fields
    When the detail view is open and 1.0 is picked
    Then the fields shown are those of 1.0

  Scenario: long fields scroll
    Given the release notes of 2.0 are 100 lines long
    When the detail view is open and Tab moves the focus to the fields
    Then PgDn scrolls the fields and the version list keeps its selection

  Scenario: typing "q" in a search does not quit
    When "/" is pressed and "qix" is typed
    Then the search reads "qix" and the TUI is still running

  Scenario: a details failure is shown
    Given the archive of "alpha" 2.0 cannot be fetched
    When the detail view opens
    Then it shows the error core reported
    And Esc returns to the list with "alpha" selected

  Scenario: reload feeds updates the catalog and every status
    Given "alpha" is selected and the feed gains "alpha" 3.0
    When "R" is pressed
    Then "alpha" shows available 3.0
    And "alpha" is still selected

  Scenario: reload feeds fetches past the cache
    When "R" is pressed
    Then the feed serves an index request

  Scenario: reload feeds while the feed is down keeps the list
    Given the feed stops answering
    When "R" is pressed
    Then the list still shows "alpha"
    And a banner says the list is from the cache and names the feed
