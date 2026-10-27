Feature: lvpm lock writes the complete resolved set

  Background:
    Given fixture packages "alpha" 1.0 and 2.0, "beta" 1.0 requiring "alpha" >= 1.0, "gamma" 1.0 and 1.5, "caraya" 1.0
    And a project with labview "2024" using only the fixture folder

  Scenario: lock records every package with exact version, hash and source
    Given the project depends on "beta" ">=1.0"
    When I run "lvpm lock"
    Then lvpm.lock lists exactly "alpha" 2.0 and "beta" 1.0
    And each entry's SHA-256 equals the SHA-256 of its fixture archive
    And each entry has source "local", because the fixture folder is a local source
    And the output says lvpm.lock was written and should be committed

  Scenario: lock prints what it decided
    Given the project depends on "beta" ">=1.0"
    When I run "lvpm lock"
    Then the output lists "+ alpha 2.0" and "+ beta 1.0"

  Scenario: dev-only packages are marked
    Given the project depends on "beta" ">=1.0" and has dev dependency "caraya" "*"
    When I run "lvpm lock"
    Then "caraya" is in lvpm.lock marked as needed only for development
    And "beta" and "alpha" are not marked

  Scenario: a package at exactly the project's LabVIEW is eligible
    Given "gamma" 1.5 needs LabVIEW 2024 and the project depends on "gamma" "*"
    When I run "lvpm lock"
    Then lvpm.lock holds "gamma" 1.5

  Scenario: a requirement no source lists is warned about and the lock is written
    Given "beta" 1.0 also requires "ni_lib_missing" >= 1.0, which no source lists
    And the project depends on "beta" ">=1.0"
    When I run "lvpm lock"
    Then the command succeeds
    And the output warns that "ni_lib_missing", required by "beta", is in no source
    And lvpm.lock records "ni_lib_missing" as unresolved

  Scenario: a conflict fails, names the manifest lines, and keeps the old lock
    Given the project depends on "beta" ">=1.0" and lvpm.lock is current
    When I change the manifest to depend on "A" "*" and "C" "1.0", where only "A" 2.0 requiring "C" >= 2.0 exists
    And I run "lvpm lock"
    Then the command fails with a report naming "A" "*" and "C" "1.0" from lvpm.toml
    And lvpm.lock is byte-identical to before

  Scenario: a direct dependency no source lists fails with a hint
    Given the project depends on "zeta" "*", which no source lists
    When I run "lvpm lock"
    Then the command fails saying "zeta" is in no configured source and suggesting "lvpm search zeta"

  Scenario: a manifest without labview is refused with an example
    Given a project manifest without labview
    When I run "lvpm lock"
    Then the command fails with: add labview = ">=<year>" under [project] in lvpm.toml
    And no lvpm.lock exists

  Scenario: unreachable archives are all reported in one error
    Given the project depends on "alpha" and "gamma", and both archives are unreachable and not cached
    When I run "lvpm lock"
    Then the command fails naming both "alpha" and "gamma"
    And no lvpm.lock is written

  Scenario: cached archives are used when the source is unreachable
    Given the archive of "alpha" 2.0 is in the cache and its source is unreachable
    And the project depends on "alpha" "*"
    When I run "lvpm lock"
    Then lvpm.lock holds "alpha" 2.0

  Scenario: locking ignores what is installed and where the world lives
    Given world one, under one temp root, has "alpha" 1.0 installed in its destination
    And world two, under a different temp root, has nothing installed
    And both depend on "beta" ">=1.0" with the same fixtures
    When I run "lvpm lock" in both worlds
    Then both lvpm.lock files are byte-identical
    And no lvpm.lock contains either temp root's path

  Scenario: a hosted package records its feed URL as the source
    Given the project lists the loopback feed under [sources] and depends on "gamma" "*" from it
    When I run "lvpm lock"
    Then the "gamma" entry has the loopback feed's index URL as its source

  Scenario: a re-lock after adding a dependency keeps the other locked versions
    Given lvpm.lock holds "alpha" 1.0 and "beta" 1.0 because "alpha" 2.0 was published after locking
    When I add "gamma" "*" to lvpm.toml and run "lvpm lock"
    Then lvpm.lock holds "alpha" 1.0, "beta" 1.0 and "gamma" 1.5
