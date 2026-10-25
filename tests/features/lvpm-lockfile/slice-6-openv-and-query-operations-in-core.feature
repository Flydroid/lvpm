Feature: Destination, index and queries live in core

  Scenario Outline: query output is unchanged
    When the characterization row "<name>" runs
    Then it still matches

    Examples:
      | name           |
      | targets        |
      | search-a       |
      | search-none    |
      | list-beta      |
      | list-empty     |
      | flags-conflict |
      | config-set     |
      | config-get     |
      | cache-add      |

  @core
  Scenario Outline: destination resolution is a function of the environment
    Given an OpEnv with <flags>, cwd <cwd> and headless <headless>
    When core resolves the destination
    Then the result is <result>

    Examples:
      | flags                          | cwd                         | headless | result                                   |
      | --prefix dest                  | anywhere                    | false    | scratch tree at dest                     |
      | --labview-version 2026 --prefix x | anywhere                 | false    | error "mutually exclusive"               |
      | none                           | a project with a venv       | false    | the venv                                 |
      | none                           | a project without a venv    | false    | error naming lvpm venv create            |
      | none                           | a project without a venv    | true     | the LabVIEW the manifest's labview names |
