Feature: Machine-readable output

  Background:
    Given a feed with "alpha" 1.0 and "beta" 0.1
    And a sandboxed user config
    And a fresh scratch folder "dest" in which "beta" 0.1 is installed
    And a fresh project whose lvpm.toml lists "alpha" = ">=1.0", with an up-to-date lock, synced into "dest"

  Scenario Outline: every covered command writes JSON Lines ending in one result
    When "lvpm --json <command>" runs in <place>
    Then every stdout line is a JSON object
    And the last line is the only one with a "result" key
    And the JSON result is <result>
    And the exit code is 0
    And the output matches the "json-<recording>" recording

    Examples:
      | command                               | place      | result                                  | recording     |
      | search alpha                          | any folder | a list with one entry, "alpha" 1.0      | search        |
      | search zzz                            | any folder | an empty list                           | search-none   |
      | search --all                          | any folder | an object whose "entries" are "alpha" 1.0 and "beta" 0.1 | search-all |
      | info alpha                            | any folder | an object whose "entry" is "alpha" 1.0  | info          |
      | list --prefix dest                    | any folder | a list with one manifest, "beta" 0.1    | list          |
      | config get venv.dir                   | any folder | {"key": "venv.dir", "values": [".lv-venv"]} | config-get |
      | config set venv.dir venvs             | any folder | {"command": "config"}                   | config-set    |
      | cache add pkgs                        | any folder | a list                                  | cache-add     |
      | install alpha --prefix dest --dry-run | any folder | {"command": "install"}                  | install       |
      | uninstall beta --prefix dest          | any folder | {"command": "uninstall"}                | uninstall     |
      | lock --check                          | the project | {"up_to_date": true}                   | lock-check    |
      | lock                                  | the project | {"command": "lock"}                    | lock          |
      | sync --prefix dest                    | the project | {"command": "sync"}                    | sync          |
      | add beta --prefix dest                | the project | {"command": "add"}                     | add           |
      | remove alpha --prefix dest            | the project | {"command": "remove"}                  | remove        |
      | venv status                           | any folder | an object with "exists" false           | venv-status   |

  # start, refresh and run-hooks have no row: they never use a venv, and the
  # harness refuses to run them on the user's LabVIEW installation. They end
  # in the same result line as the rows below.
  @labview
  Scenario Outline: the commands that need a LabVIEW
    Given <setup>
    When "lvpm --json <command>" runs in the project
    Then the last line is the only one with a "result" key and it is {"command": "<name>"}
    And the exit code is 0

    Examples:
      | setup                                          | command                        | name    |
      | the project has a venv on the detected LabVIEW | launch                         | launch  |
      | a venv with beta installed                     | relink --all                   | relink  |

  @labview
  Scenario: venv create on a project with no venv
    Given the project needs the detected LabVIEW and has no venv
    When "lvpm --json venv create" runs in the project
    Then the last line is the only one with a "result" key and it is {"command": "venv"}
    And the exit code is 0

  Scenario: targets is a list, with no recording since it names this machine's LabVIEWs
    When "lvpm --json targets" runs in any folder
    Then every stdout line is a JSON object
    And the JSON result is a list
    And the exit code is 0

  Scenario: a failure ends in one error object and exit code 1
    When "lvpm --json info nothing" runs in any folder
    Then the last stdout line is the only one with an "error" key
    And its message names the sources searched
    And the exit code is 1

  Scenario: events followed by a failure
    Given the archive of "alpha" 1.0 is corrupt
    When "lvpm --json install alpha --prefix dest" runs in any folder
    Then stdout has a "phase_started" event for download before the last line
    And the last line is the only one with an "error" key
    And the exit code is 1

  Scenario: a stale lock fails the check
    Given lvpm.toml instead lists "alpha" = ">=1.0" and "beta" = ">=0.1"
    When "lvpm --json lock --check" runs in the project
    Then the last line is an error saying the lock is out of date
    And the exit code is 1

  Scenario: malformed arguments under --json
    When "lvpm --json install --no-such-flag" runs
    Then stdout is empty
    And the exit code is 2

  Scenario: install progress reports its phases in order, skipped ones too
    When "lvpm --json install alpha --prefix dest" runs in any folder
    Then the "phase" values of its phase events are, in order, resolve, download, unpack, relink, hooks, refresh
    And relink, hooks and refresh are "phase_skipped" events with reason "scratch_destination"

  @labview
  Scenario: a command that would prompt fails instead under --json
    Given the project has a venv on the detected LabVIEW
    When "lvpm --json venv remove" runs in the project
    Then the last line is an error that names --yes
    And the exit code is 1
    And the venv still exists

  @labview
  Scenario: without --json the prompt stays
    Given the project has a venv on the detected LabVIEW
    When "lvpm venv remove" runs in the project and "n" is typed
    Then it asked "[y/N]"
    And the venv still exists

  Scenario: the diagnostics do not take --json
    When "lvpm --json vi-probe x.vi" runs
    Then it fails as a usage error with exit code 2

  Scenario: without --json nothing changes
    When every characterization row runs
    Then each still matches

  Scenario: the exit codes are documented
    When the README's exit-code table is read
    Then it lists 0, 1 and 2, and each has a scenario in this feature
