Feature: The current CLI's output is recorded so later changes can be compared against it

  # How the rows below are read:
  #  - "files" names a directory, relative to the project, whose files are
  #    listed and recorded as tree.txt after the command ("-" lists nothing).
  #  - Every setup runs its own lvpm commands in a fresh sandbox.
  #  - @labview rows need LVPM_TEST_LABVIEW=1 and a real LabVIEW. They work
  #    inside a venv in the sandbox, never on the LabVIEW installation:
  #    `run-hooks`, `refresh` and `start` name no destination here, so they
  #    record lvpm's refusal; passing --labview-version would act on the
  #    installation, which no row below does.

  Background:
    Given fixture packages "alpha" 1.0 and 2.0, "beta" 1.0 requiring "alpha" >= 1.0, and "hooked" 1.0 declaring a PostInstall hook
    And a loopback feed listing the same packages

  Scenario Outline: a command's output matches its recording
    Given <setup>
    When I run "<command>"
    And I list the files under "<files>"
    Then stdout, stderr and the exit code equal the recording "<name>"

    Examples:
      | setup                                                   | command                                          | files | name                     |
      | a project using only the loopback feed                  | lvpm search a                                    | -     | search-a                 |
      | a project using only the loopback feed                  | lvpm search zzz                                  | -     | search-none              |
      | a project using only the loopback feed                  | lvpm install beta --dry-run --prefix dest        | dest  | install-dry-run          |
      | a project using only the loopback feed                  | lvpm install beta --prefix dest                  | dest  | install-beta             |
      | a project using only the loopback feed                  | lvpm install hooked --prefix dest                | dest  | install-hooked           |
      | beta installed in dest                                  | lvpm list --prefix dest                          | -     | list-beta                |
      | an empty dest                                           | lvpm list --prefix dest                          | -     | list-empty               |
      | beta installed in dest                                  | lvpm uninstall beta --prefix dest                | dest  | uninstall-beta           |
      | an empty dest                                           | lvpm uninstall beta --prefix dest                | dest  | uninstall-missing        |
      | a project using only the loopback feed                  | lvpm install does_not_exist --prefix dest        | dest  | install-missing          |
      | no project                                              | lvpm list --labview-version 2026 --prefix dest   | -     | flags-conflict           |
      | no project                                              | lvpm config set venv.dir .venv                   | -     | config-set               |
      | venv.dir set to .venv                                   | lvpm config get venv.dir                         | -     | config-get               |
      | a folder with alpha 1.0                                 | lvpm cache add folder                            | -     | cache-add                |
      | no project, default feeds via the loopback proxy        | lvpm search a                                    | -     | search-a-noproject       |
      | no project, default feeds via the loopback proxy        | lvpm install beta --prefix dest                  | dest  | install-beta-noproject   |
      | no project, beta installed in dest                      | lvpm list --prefix dest                          | -     | list-beta-noproject      |
      | no project, beta installed in dest                      | lvpm uninstall beta --prefix dest                | dest  | uninstall-beta-noproject |
      | a project using the faulty feed                         | lvpm install corrupt --prefix dest               | dest  | install-corrupt          |
      | a project using the faulty feed                         | lvpm install tampered --prefix dest              | dest  | install-tampered         |
      | a project using the faulty feed                         | lvpm install missing --prefix dest               | dest  | install-archive-404      |
      | a project using the catalog feed                        | lvpm search bulk                                 | -     | search-over-cap          |

  Scenario: feed progress lines are recorded, first run and unchanged
    Given a project using only the loopback feed and an empty cache
    When I run "lvpm search a" twice, the second time with --refresh
    Then both outputs equal the recordings "feed-first-run" and "feed-unchanged"

  Scenario: a changed recording is detected
    Given a project using only the loopback feed
    And the recording "search-a" with one byte altered
    When I run "lvpm search a" and compare it with the altered recording "search-a"
    Then the comparison fails

  # The recording keeps only the line for LabVIEW 2026, so that other
  # installations on a machine do not matter.
  @labview
  Scenario: lvpm targets lists the LabVIEW 2026 installation
    Given no project
    And a LabVIEW 2026 installation
    When I run "lvpm targets"
    Then the output lines mentioning "LabVIEW 2026" equal the recording "targets"

  @labview
  Scenario Outline: LabVIEW-bound command output matches its recording
    Given <setup>
    When I run "<command>"
    And I list the files under "<files>"
    Then stdout, stderr and the exit code equal the recording "<name>"

    Examples:
      | setup                                         | command                                   | files     | name              |
      | a project requiring LabVIEW 2026              | lvpm venv create --labview-version 2026   | .lv-venv  | venv-create       |
      | a venv for LabVIEW 2026                       | lvpm venv status                          | -         | venv-status       |
      | a venv for LabVIEW 2026                       | lvpm install beta                         | .lv-venv  | venv-install-beta |
      | a venv with beta installed                    | lvpm relink --all                         | .lv-venv  | relink-all        |
      | a venv with hooked installed                  | lvpm run-hooks hooked                     | -         | run-hooks         |
      | a venv for LabVIEW 2026                       | lvpm refresh                              | -         | refresh           |
      | a venv for LabVIEW 2026                       | lvpm start                                | -         | start             |
      | a venv for LabVIEW 2026                       | lvpm launch                               | -         | launch            |
      | a venv for LabVIEW 2026                       | lvpm venv remove --yes                    | .lv-venv  | venv-remove-yes   |

  @labview
  Scenario Outline: the venv remove prompt is recorded
    Given a project with a venv
    When I run "lvpm venv remove" and answer "<answer>"
    And I list the files under ".lv-venv"
    Then stdout, stderr and the exit code equal the recording "<name>"

    Examples:
      | answer | name                   |
      | n      | venv-remove-no         |
      | y      | venv-remove-yes-prompt |
