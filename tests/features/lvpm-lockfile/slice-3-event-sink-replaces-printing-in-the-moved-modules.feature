Feature: Core reports progress as structured events and the CLI prints them

  Scenario: feed progress still renders byte for byte
    Given the recordings "feed-first-run" and "feed-unchanged"
    When those characterization rows run
    Then both still match

  @core
  Scenario: loading a feed emits a begin and an end event
    Given a project using only the loopback feed and an empty cache
    When core loads the index with a recording sink
    Then the sink received FeedCheckStart for the feed URL, then FeedChecked with outcome "downloaded"
    And every event serialises to JSON whose string fields are a package name, version, URL, path or an enumerated value

  @labview
  Scenario: the venv remove prompt is asked by the CLI
    Given a project with a venv
    When I run "lvpm venv remove" and answer "n"
    Then the output equals the recording "venv-remove-no"
    And the venv still exists
