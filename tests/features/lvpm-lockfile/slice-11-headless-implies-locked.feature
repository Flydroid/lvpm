Feature: On a headless machine a missing or outdated lock fails the job

  Background:
    Given LV_RTE_HEADLESS is set
    And a project with labview "2024" and a scratch destination "dest"

  Scenario Outline: sync and bare install fail on a missing or stale lock
    Given the lock is <state>
    When I run "<command> --prefix dest"
    Then the first output line says headless mode implies --locked
    And the command fails with "<hint>"
    And dest and lvpm.lock are unchanged

    Examples:
      | state   | command      | hint                                                  |
      | missing | lvpm sync    | no lvpm.lock: run lvpm lock locally and commit it, or pass --no-locked |
      | missing | lvpm install | no lvpm.lock: run lvpm lock locally and commit it, or pass --no-locked |
      | stale   | lvpm sync    | lvpm.lock is out of date                              |
      | stale   | lvpm install | lvpm.lock is out of date                              |

  Scenario: a current lock syncs and still prints the note
    Given a current lock
    When I run "lvpm sync --prefix dest"
    Then the first output line says headless mode implies --locked
    And dest matches the lock

  Scenario: --no-locked re-locks
    Given a stale lock
    When I run "lvpm sync --no-locked --prefix dest"
    Then lvpm.lock is rewritten and dest matches it

  Scenario: without LV_RTE_HEADLESS there is no note
    Given LV_RTE_HEADLESS is not set and a stale lock
    When I run "lvpm sync --prefix dest"
    Then no output line mentions headless mode
    And lvpm.lock is rewritten

  Scenario: --frozen still applies
    Given a stale lock
    When I run "lvpm sync --frozen --prefix dest"
    Then dest matches lvpm.lock as it was

  Scenario Outline: lock-editing commands are not affected
    Given a stale lock
    When I run "<command>"
    Then it succeeds and lvpm.lock is current afterwards

    Examples:
      | command                        |
      | lvpm lock                      |
      | lvpm add gamma --prefix dest   |
      | lvpm remove beta --prefix dest |
