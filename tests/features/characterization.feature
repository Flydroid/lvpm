Feature: The current CLI's output is recorded so later changes can be compared against it

  Background:
    Given fixture packages "alpha" 1.0 and 2.0, "beta" 1.0 requiring "alpha" >= 1.0, and "hooked" 1.0 declaring a PostInstall hook
    And a loopback feed listing the same packages

  Scenario Outline: a command's output matches its recording
    Given <setup>
    When I run "<command>"
    Then stdout, stderr and the exit code equal the recording "<name>"

    Examples:
      | setup                                         | command                                          | name                 |
      | a project using only the loopback feed       | lvpm search a                                    | search-a             |
      | a project using only the loopback feed       | lvpm search zzz                                  | search-none          |
      | a project using only the loopback feed       | lvpm install beta --dry-run --prefix dest        | install-dry-run      |
      | a project using only the loopback feed       | lvpm install beta --prefix dest                  | install-beta         |
      | a project using only the loopback feed       | lvpm install hooked --prefix dest                | install-hooked       |
      | beta installed in dest                       | lvpm list --prefix dest                          | list-beta            |
      | an empty dest                                | lvpm list --prefix dest                          | list-empty           |
      | beta installed in dest                       | lvpm uninstall beta --prefix dest                | uninstall-beta       |
      | an empty dest                                | lvpm uninstall beta --prefix dest                | uninstall-missing    |
      | a project using only the loopback feed       | lvpm install does_not_exist --prefix dest        | install-missing      |
      | no project                                   | lvpm list --labview-version 2026 --prefix dest   | flags-conflict       |
      | no project                                   | lvpm config set venv.dir .venv                   | config-set           |
      | venv.dir set to .venv                        | lvpm config get venv.dir                         | config-get           |
      | a folder with alpha 1.0                      | lvpm cache add folder                            | cache-add            |
      | no project, seeded default feeds             | lvpm search a                                    | search-a-noproject   |
      | no project, seeded default feeds             | lvpm install beta --prefix dest                  | install-beta-noproject |
      | no project, beta installed in dest           | lvpm list --prefix dest                          | list-beta-noproject  |
      | no project, beta installed in dest           | lvpm uninstall beta --prefix dest                | uninstall-beta-noproject |

  Scenario: feed progress lines are recorded, first run and unchanged
    Given a project using only the loopback feed and an empty cache
    When I run "lvpm search a" twice, the second time with --refresh
    Then both outputs equal the recordings "feed-first-run" and "feed-unchanged"

  Scenario: a changed recording is detected
    Given the recording "search-a" with one byte altered
    When the "search-a" comparison runs
    Then the comparison fails

  # `lvpm targets` lists the host's LabVIEW installations, so its recording
  # belongs to a LabVIEW machine. Every other row works inside a venv in the
  # world, never on the installation itself. `run-hooks`, `refresh` and
  # `start` name no destination here, so they record the refusal; passing
  # --labview-version would act on the installation, which no row does.
  @labview
  Scenario Outline: LabVIEW-bound command output matches its recording
    Given <setup>
    When I run "<command>"
    And I close any LabVIEW the command started on the venv
    Then stdout, stderr and the exit code equal the recording "<name>"

    Examples:
      | setup                                         | command                                   | name              |
      | no project                                    | lvpm targets                              | targets           |
      | a project requiring LabVIEW 2026              | lvpm venv create --labview-version 2026   | venv-create       |
      | a venv for LabVIEW 2026                       | lvpm venv status                          | venv-status       |
      | a venv for LabVIEW 2026                       | lvpm install beta                         | venv-install-beta |
      | a venv with beta installed                    | lvpm relink --all                         | relink-all        |
      | a venv with hooked installed                  | lvpm run-hooks hooked                     | run-hooks         |
      | a venv for LabVIEW 2026                       | lvpm refresh                              | refresh           |
      | a venv for LabVIEW 2026                       | lvpm start                                | start             |
      | a venv for LabVIEW 2026                       | lvpm launch                               | launch            |
      | a venv for LabVIEW 2026                       | lvpm venv remove --yes                    | venv-remove-yes   |

  @labview
  Scenario Outline: the venv remove prompt is recorded
    Given a project with a venv
    When I run "lvpm venv remove" and answer "<answer>"
    Then stdout, stderr and the exit code equal the recording "<name>"

    Examples:
      | answer | name           |
      | n      | venv-remove-no |
      | y      | venv-remove-yes-prompt |
