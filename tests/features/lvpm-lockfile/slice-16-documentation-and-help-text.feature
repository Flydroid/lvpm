Feature: The lock workflow is documented where users look

  Scenario: every subcommand and flag has help text and a README entry
    When the CLI's command tree is enumerated
    Then every subcommand and every flag has a non-empty help text
    And every subcommand and every lock-related flag appears in the README

  Scenario: install's help says what it does in a project
    When I run "lvpm install --help"
    Then it says that in a project bare install is sync and a named install is add

  Scenario: the README shows how to quote constraints
    When the README's add section is read
    Then it shows a quoted example "lvpm add \"gamma@>=0.5\"" for PowerShell and cmd
