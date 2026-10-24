Feature: Every package-management operation lives in core

  @labview
  Scenario Outline: maintenance, venv and launch output is unchanged
    When the characterization row "<name>" runs
    Then it still matches

    Examples:
      | name            |
      | relink-all      |
      | run-hooks       |
      | refresh         |
      | start           |
      | launch          |
      | venv-create     |
      | venv-status     |
      | venv-remove-yes |
      | venv-remove-no  |

  Scenario: core contains no terminal I/O at all
    When clippy runs on lvpm-core with warnings as errors
    Then no print lint and no disallowed stdin, stdout or stderr call is reported
