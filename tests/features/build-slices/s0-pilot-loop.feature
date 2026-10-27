Feature: The pilot loop: build, publish, install, mass compile
  The scenarios every build slice is judged by. Pilot A (HSE Logger) and
  pilot B (OpenSerializer) come from tests/fixtures/pilots, each archive
  checked against its SHA-256 in SHA256SUMS; README.md next to this file says where they came from.

  Scenario: the pilot archives match their checksums
    Given the pilot archives
    Then each pilot archive's SHA-256 matches its checksum in SHA256SUMS

  Scenario: a pilot archive that differs from its checksum is refused
    Given a copy of the pilot archives with one byte of "hse_lib_hse_logger-2.2.2.114.vip" changed
    When the copies are checked against their checksums
    Then the check fails naming "hse_lib_hse_logger-2.2.2.114.vip"

  # Interactive until the CI image can launch a venv LabVIEW and run LabVIEWCLI.
  @labview @interactive-labview
  Scenario: both pilots install into a fresh venv and mass compile there
    Given a LabVIEW 2026 installation without the pilots
    And project B depending on both pilots, with a folder feed of the pilot archives as its only source
    And a venv for project B
    When project B is installed
    Then the venv holds both pilots and the packages OpenSerializer needs
    And mass compiling the venv's packages in a LabVIEW on the venv succeeds

  @labview @pending-s1
  Scenario: lvpm build turns pilot A's library into a .vip
    Given pilot A's repository at its pinned commit
    When lvpm build runs in pilot A
    Then dist holds a .vip with only the members of pilot A's library

  @labview @pending-s2
  Scenario: lvpm publish puts pilot A's .vip in a folder feed project B installs from
    Given pilot A built with lvpm build
    When lvpm publish puts it in a folder feed
    And project B is installed from that feed
    Then mass compiling the venv's packages in a LabVIEW on the venv succeeds
