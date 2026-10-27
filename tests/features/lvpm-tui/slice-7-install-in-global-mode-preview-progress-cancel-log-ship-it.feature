Feature: Install with a preview, progress and a log

  Background:
    Given the TUI runs at 80x24 on a scratch destination
    And a source with "alpha" 1.0 and 2.0, where "alpha" requires "beta" 1.0

  Scenario: install shows the dry-run plan and runs only after confirmation
    When "alpha" is selected and "i" is pressed
    Then the preview lists "alpha" 2.0 and "beta" 1.0 as new
    And nothing is installed yet
    When Enter is pressed
    Then the progress view shows the phases resolve, download, unpack, relink, hooks, refresh in order
    And relink, hooks and refresh are shown as skipped for a scratch destination
    And the result says done and "alpha" is installed
    When Enter is pressed
    Then the list shows "alpha" as "installed" and "alpha" is selected

  Scenario: the preview is computed with a cancel
    Given the feed holds archive downloads
    When "alpha" is selected and "i" is pressed
    Then the screen says the plan is being computed
    When Esc is pressed
    Then the list is shown with "alpha" selected and nothing is installed

  Scenario: a plan that cannot be made is shown
    Given the archive of "alpha" 2.0 cannot be fetched
    When "alpha" is selected and "i" is pressed
    Then the screen shows core's error
    And Esc returns to the list with "alpha" selected

  Scenario: the preview names a dependency no source has
    Given "alpha" 2.0 instead requires "omega" 1.0, which no source has
    When "alpha" is selected and "i" is pressed
    Then the preview says no source has "omega"

  Scenario: Esc on the preview changes nothing
    When "alpha" is selected, "i" is pressed and Esc is pressed
    Then nothing is installed

  Scenario: a downgrade asks twice and runs with allow-downgrade
    Given "alpha" 2.0 is installed
    When version 1.0 of "alpha" is picked in the details and "i" is pressed
    Then the preview marks "alpha" as a downgrade
    When Enter is pressed
    Then a second confirmation asks for "y"
    When "y" is pressed
    Then "alpha" 1.0 is installed

  Scenario: Esc at the second downgrade confirmation changes nothing
    Given "alpha" 2.0 is installed
    When version 1.0 of "alpha" is picked, "i" is pressed, Enter is pressed and Esc is pressed
    Then "alpha" 2.0 is still installed

  Scenario Outline: cancel during resolve or download stops before any file is written
    When an install of "alpha" is confirmed and paused at <phase>
    And "<key>" is pressed
    Then the result says cancelled
    And no file of "alpha" or "beta" is in the destination

    Examples:
      | phase    | key    |
      | resolve  | c      |
      | download | c      |
      | download | Esc    |
      | download | Ctrl-C |

  Scenario: cancel is refused once unpack has started
    When an install is confirmed and paused at unpack
    And "c" is pressed
    Then the progress view says cancel is no longer possible
    When the install is released
    Then the install finishes and "alpha" is installed

  Scenario Outline: other input is refused while an operation runs
    When an install is confirmed and paused at download
    And "<key>" is pressed
    Then the progress view stays
    And the footer says the key waits until the operation ends

    Examples:
      | key |
      | /   |
      | q   |
      | i   |

  Scenario: a resize keeps the progress
    When an install is confirmed and paused at download
    And the terminal is resized to 100x30
    Then the progress view shows resolve done and download running

  Scenario: a failed install shows the error and the destination as it is
    Given "beta" 1.0 installs and then the archive of "alpha" 2.0 is corrupt
    When an install of "alpha" is confirmed
    Then the result shows core's error naming "alpha" 2.0
    When Enter is pressed
    Then the list shows "beta" as "installed" and "alpha" as "not installed"
    And "L" shows the operation's log, ending in the error

  Scenario: every operation is appended to tui.log
    When an install runs to the end
    Then tui.log ends with an entry whose first line has an RFC 3339 UTC timestamp and names the install of "alpha"
    And the entry's lines equal the in-session log of that operation

  Scenario Outline: a failed or cancelled operation is logged too
    When an install of "alpha" <ends>
    Then tui.log ends with an entry naming the install of "alpha" and saying <outcome>

    Examples:
      | ends                          | outcome   |
      | fails on a corrupt archive    | failed    |
      | is cancelled during download  | cancelled |

  Scenario: tui.log rotates at 10 MiB
    Given tui.log is 10 MiB and tui.log.1 holds an older backup
    When an operation is logged
    Then tui.log.1 holds the 10 MiB file
    And tui.log holds only the new entry

  Scenario: tui.log under 10 MiB is appended to
    Given tui.log is 1 byte under 10 MiB
    When an operation is logged
    Then tui.log.1 does not exist and tui.log ends with the new entry

  Scenario: an unwritable log folder does not stop the TUI
    Given the log folder cannot be written
    When an install runs to the end
    Then the install succeeds
    And the footer says tui.log could not be written
