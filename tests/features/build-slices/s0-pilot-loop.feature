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

  # The published package also ships the probes, VI templates and version file
  # from outside the library; lvpm build packages the library's members only.
  @pilot-source
  Scenario: pilot A's package holds its library's members, as the published package does
    Given pilot A's repository at its pinned commit
    When lvpm build runs in pilot A
    Then the package's files are the vi.lib files of File Group 0 of "hse_lib_hse_logger-2.2.2.114.vip", except the ones its library does not list:
      | vi.lib/Hampel Software Engineering/HSE Logger/_VI Templates/HSE Logger - Log Handlers and Levels.vi |
      | vi.lib/Hampel Software Engineering/HSE Logger/_VI Templates/HSE Logger - UE Handler Quickstart.vi   |
      | vi.lib/Hampel Software Engineering/HSE Logger/versioninfo.txt                                       |
      | vi.lib/_probes/Hampel Software Engineering/HSE Logger/HSE Logger Custom Probe - DEBUG (Error).vi   |
      | vi.lib/_probes/Hampel Software Engineering/HSE Logger/HSE Logger Custom Probe - DEBUG (String).vi  |
      | vi.lib/_probes/Hampel Software Engineering/HSE Logger/HSE Logger Custom Probe - DEBUG (Variant).vi |

  @labview @interactive-labview @pilot-source
  Scenario: lvpm build turns pilot A's library into a .lvpkg project B installs and mass compiles
    Given a LabVIEW 2026 installation without the pilots
    And pilot A built with lvpm build
    And project B depending on both pilots, with a folder feed of the built pilot A and the other pilot archives
    And a venv for project B
    When project B is installed
    Then the venv holds both pilots and the packages OpenSerializer needs
    And mass compiling the venv's packages in a LabVIEW on the venv succeeds

  @labview @interactive-labview @pilot-source
  Scenario: lvpm publish puts pilot A's .lvpkg in a folder feed project B installs from
    Given a LabVIEW 2026 installation without the pilots
    And pilot A built with lvpm build
    And lvpm publish has put it in an empty folder feed
    And project B depending on both pilots, with that feed and the other pilot archives as its sources
    And a venv for project B
    When project B is installed
    Then the venv holds both pilots and the packages OpenSerializer needs
    And mass compiling the venv's packages in a LabVIEW on the venv succeeds
