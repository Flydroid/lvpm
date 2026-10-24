@core
Feature: A long operation can be cancelled until it starts writing files

  Background:
    Given a project using only the loopback feed, which can pause a download until released
    And a scratch destination

  Scenario: cancel at the first resolve event writes nothing
    When core installs "beta" and the token is cancelled when the first resolve event arrives
    Then the result is "cancelled"
    And the destination holds no package files and no install manifests

  Scenario: cancel while a download is paused writes nothing
    When core installs "beta", the feed pauses the archive download, the token is cancelled and the feed is released
    Then the result is "cancelled"
    And the destination holds no package files

  Scenario Outline: cancel after unpack has begun is refused and the install completes
    When core installs "beta" and the token is cancelled when the first <phase> event arrives
    Then cancel reports it was not accepted
    And the install succeeds and "beta" is installed

    Examples:
      | phase   |
      | unpack  |
      | relink  |
      | hooks   |
      | refresh |

  Scenario: cancel during lock leaves both files untouched
    When core locks the project and the token is cancelled while an archive is being hashed
    Then the result is "cancelled"
    And lvpm.toml and lvpm.lock are byte-identical to before

  Scenario: cancel during sync before the first change leaves the destination untouched
    When core syncs and the token is cancelled while archives are being fetched
    Then the result is "cancelled"
    And the destination is unchanged
