//! Which scenarios the acceptance harness runs, from their tags and the
//! environment.

pub const LABVIEW_TAG: &str = "labview";
/// A scenario that needs a later slice, tagged `@pending-<slice>`. It is not
/// run until that slice removes the tag.
pub const PENDING_TAG_PREFIX: &str = "pending-";
/// A scenario that would change the user's own LabVIEW installation: a sync
/// takes out every package its lock does not list, so none may run against
/// one. The harness never runs these, whatever else is set.
pub const UNSAFE_TAG: &str = "unsafe";
/// A `@labview` scenario that needs a LabVIEW someone could click through, not
/// a headless one: it is not run where the LabVIEW is headless (a container),
/// and a headless copy of it runs there instead, where one exists.
pub const INTERACTIVE_TAG: &str = "interactive-labview";
/// A `@labview` scenario that drives the LabVIEW installation itself, the one
/// LabVIEW every worktree on the machine shares: one run at a time uses it.
pub const GLOBAL_TAG: &str = "global-labview";
/// A scenario that builds pilot A from its source. The source may not be
/// redistributed, so it is not in this repository: the scenario runs only
/// when `LVPM_TEST_PILOT_A` names a checkout of it.
pub const PILOT_SOURCE_TAG: &str = "pilot-source";
/// A checkout of pilot A at its pinned commit, for the `@pilot-source` scenarios.
pub const PILOT_A_ENV_VAR: &str = "LVPM_TEST_PILOT_A";

/// Whether a scenario runs, and if not, why.
#[derive(Debug, PartialEq, Eq)]
pub enum Selection {
    Run,
    Unsafe,
    Pending,
    /// A `@labview` scenario without `LVPM_TEST_LABVIEW=1`.
    NeedsLabview,
    /// Any other scenario under `LVPM_TEST_ONLY_LABVIEW=1`.
    NotLabview,
    /// An `@interactive-labview` scenario under `LVPM_TEST_LABVIEW_HEADLESS=1`.
    Interactive,
    /// A `@global-labview` scenario while another run, or the user, has the
    /// global LabVIEW.
    GlobalBusy,
    /// A `@pilot-source` scenario without `LVPM_TEST_PILOT_A`.
    NeedsPilotSource,
}

/// `test_labview`: `LVPM_TEST_LABVIEW=1`; `only_labview`: `LVPM_TEST_ONLY_LABVIEW=1`;
/// `headless_labview`: `LVPM_TEST_LABVIEW_HEADLESS=1`; `global_busy`: this run
/// could not have the global LabVIEW; `pilot_source`: `LVPM_TEST_PILOT_A` is set.
pub fn select(
    tags: &[String],
    test_labview: bool,
    only_labview: bool,
    headless_labview: bool,
    global_busy: bool,
    pilot_source: bool,
) -> Selection {
    let has = |wanted: &str| tags.iter().any(|tag| tag == wanted);
    match () {
        _ if has(UNSAFE_TAG) => Selection::Unsafe,
        _ if tags.iter().any(|tag| tag.starts_with(PENDING_TAG_PREFIX)) => Selection::Pending,
        _ if has(LABVIEW_TAG) && !test_labview => Selection::NeedsLabview,
        _ if has(PILOT_SOURCE_TAG) && !pilot_source => Selection::NeedsPilotSource,
        _ if has(INTERACTIVE_TAG) && headless_labview => Selection::Interactive,
        _ if has(GLOBAL_TAG) && global_busy => Selection::GlobalBusy,
        _ if !has(LABVIEW_TAG) && only_labview => Selection::NotLabview,
        _ => Selection::Run,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn only_labview_keeps_the_labview_scenarios_and_never_the_unsafe_ones() {
        assert_eq!(
            select(&tags(&["labview"]), true, true, false, false, true),
            Selection::Run
        );
        assert_eq!(
            select(&tags(&[]), true, true, false, false, true),
            Selection::NotLabview
        );
        assert_eq!(
            select(
                &tags(&["labview", "unsafe"]),
                true,
                true,
                false,
                false,
                true
            ),
            Selection::Unsafe
        );
        assert_eq!(
            select(
                &tags(&["labview", "pending-9"]),
                true,
                true,
                false,
                false,
                true
            ),
            Selection::Pending
        );
    }

    #[test]
    fn without_only_labview_every_safe_scenario_runs_and_labview_ones_need_the_flag() {
        assert_eq!(
            select(&tags(&[]), false, false, false, false, true),
            Selection::Run
        );
        assert_eq!(
            select(&tags(&["labview"]), false, false, false, false, true),
            Selection::NeedsLabview
        );
        assert_eq!(
            select(&tags(&["labview"]), true, false, false, false, true),
            Selection::Run
        );
        assert_eq!(
            select(&tags(&["unsafe"]), true, false, false, false, true),
            Selection::Unsafe
        );
    }

    #[test]
    fn a_headless_labview_skips_the_interactive_scenarios_and_runs_the_rest() {
        let interactive = tags(&["labview", "interactive-labview"]);
        assert_eq!(
            select(&interactive, true, true, true, false, true),
            Selection::Interactive
        );
        assert_eq!(
            select(&interactive, true, true, false, false, true),
            Selection::Run
        );
        assert_eq!(
            select(&tags(&["labview"]), true, true, true, false, true),
            Selection::Run
        );
    }

    #[test]
    fn a_global_labview_scenario_waits_for_a_run_that_has_the_global_labview() {
        let global = tags(&["labview", "global-labview"]);
        assert_eq!(
            select(&global, true, false, false, true, true),
            Selection::GlobalBusy
        );
        assert_eq!(
            select(&global, true, false, false, false, true),
            Selection::Run
        );
        assert_eq!(
            select(&tags(&["labview"]), true, false, false, true, true),
            Selection::Run
        );
        assert_eq!(
            select(&global, false, false, false, true, true),
            Selection::NeedsLabview
        );
    }

    #[test]
    fn a_pilot_source_scenario_needs_the_checkout() {
        let pilot = tags(&["pilot-source"]);
        assert_eq!(
            select(&pilot, false, false, false, false, false),
            Selection::NeedsPilotSource
        );
        assert_eq!(
            select(&pilot, false, false, false, false, true),
            Selection::Run
        );
    }
}
