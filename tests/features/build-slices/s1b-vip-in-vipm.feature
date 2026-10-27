Feature: lvpm builds pilot B
  Runs when LVPM_TEST_PILOT_B names a checkout of
  github.com/LabVIEW-Open-Source/Serializer at 723d7a67 (tag 1.2.1).
  README.md next to this file says how to make one.

  @pilot-b-source
  Scenario: pilot B builds from its pinned checkout
    Given pilot B's repository at its pinned commit
    And an lvpm.toml for it written by the scenario
    When lvpm build runs
    Then dist holds "labview_open_source_lib_serializer-1.2.1.16.vip"
    And its Requires names the same packages as the published archive's
