Feature: The lock workflow is documented where users look

  Scenario: every subcommand and flag has help text and a usage-guide entry
    When the CLI's command tree is enumerated
    Then every subcommand and every flag has a non-empty help text
    And every subcommand and every lock-related flag appears in the usage guide

  Scenario: install's help says what it does in a project
    When I run "lvpm install --help"
    Then it says that in a project bare install is sync and a named install is add

  Scenario: venv create's help names the manifest key that exists
    When I run "lvpm venv create --help"
    Then it names the manifest key "labview"

  Scenario: the usage guide shows how to quote constraints
    When the usage guide's add section is read
    Then it shows a quoted example "lvpm add \"gamma@>=0.5\"" for PowerShell and cmd
