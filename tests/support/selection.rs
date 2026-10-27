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
}

/// `test_labview`: `LVPM_TEST_LABVIEW=1`; `only_labview`: `LVPM_TEST_ONLY_LABVIEW=1`;
/// `headless_labview`: `LVPM_TEST_LABVIEW_HEADLESS=1`.
pub fn select(
    tags: &[String],
    test_labview: bool,
    only_labview: bool,
    headless_labview: bool,
) -> Selection {
    let has = |wanted: &str| tags.iter().any(|tag| tag == wanted);
    match () {
        _ if has(UNSAFE_TAG) => Selection::Unsafe,
        _ if tags.iter().any(|tag| tag.starts_with(PENDING_TAG_PREFIX)) => Selection::Pending,
        _ if has(LABVIEW_TAG) && !test_labview => Selection::NeedsLabview,
        _ if has(INTERACTIVE_TAG) && headless_labview => Selection::Interactive,
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
            select(&tags(&["labview"]), true, true, false),
            Selection::Run
        );
        assert_eq!(select(&tags(&[]), true, true, false), Selection::NotLabview);
        assert_eq!(
            select(&tags(&["labview", "unsafe"]), true, true, false),
            Selection::Unsafe
        );
        assert_eq!(
            select(&tags(&["labview", "pending-9"]), true, true, false),
            Selection::Pending
        );
    }

    #[test]
    fn without_only_labview_every_safe_scenario_runs_and_labview_ones_need_the_flag() {
        assert_eq!(select(&tags(&[]), false, false, false), Selection::Run);
        assert_eq!(
            select(&tags(&["labview"]), false, false, false),
            Selection::NeedsLabview
        );
        assert_eq!(
            select(&tags(&["labview"]), true, false, false),
            Selection::Run
        );
        assert_eq!(
            select(&tags(&["unsafe"]), true, false, false),
            Selection::Unsafe
        );
    }

    #[test]
    fn a_headless_labview_skips_the_interactive_scenarios_and_runs_the_rest() {
        let interactive = tags(&["labview", "interactive-labview"]);
        assert_eq!(
            select(&interactive, true, true, true),
            Selection::Interactive
        );
        assert_eq!(select(&interactive, true, true, false), Selection::Run);
        assert_eq!(
            select(&tags(&["labview"]), true, true, true),
            Selection::Run
        );
    }
}
