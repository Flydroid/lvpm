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

/// What the environment of a run allows.
#[derive(Debug, Default)]
pub struct Env {
    /// `LVPM_TEST_LABVIEW=1`.
    pub test_labview: bool,
    /// `LVPM_TEST_ONLY_LABVIEW=1`.
    pub only_labview: bool,
    /// `LVPM_TEST_LABVIEW_HEADLESS=1`.
    pub headless_labview: bool,
    /// This run could not have the global LabVIEW.
    pub global_busy: bool,
    /// `LVPM_TEST_PILOT_A` is set.
    pub pilot_a: bool,
}

pub fn select(tags: &[String], env: &Env) -> Selection {
    let has = |wanted: &str| tags.iter().any(|tag| tag == wanted);
    match () {
        _ if has(UNSAFE_TAG) => Selection::Unsafe,
        _ if tags.iter().any(|tag| tag.starts_with(PENDING_TAG_PREFIX)) => Selection::Pending,
        _ if has(LABVIEW_TAG) && !env.test_labview => Selection::NeedsLabview,
        _ if has(PILOT_SOURCE_TAG) && !env.pilot_a => Selection::NeedsPilotSource,
        _ if has(INTERACTIVE_TAG) && env.headless_labview => Selection::Interactive,
        _ if has(GLOBAL_TAG) && env.global_busy => Selection::GlobalBusy,
        _ if !has(LABVIEW_TAG) && env.only_labview => Selection::NotLabview,
        _ => Selection::Run,
    }
}

/// The report line for `count` scenarios tagged `@<tag>` left out because
/// `env_var` names no checkout of pilot `pilot`.
pub fn pilot_not_run_line(count: usize, tag: &str, env_var: &str, pilot: &str) -> String {
    format!("{count} @{tag} scenarios not run (set {env_var} to a checkout of pilot {pilot})")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn only_labview_keeps_the_labview_scenarios_and_never_the_unsafe_ones() {
        let env = Env {
            test_labview: true,
            only_labview: true,
            ..Env::default()
        };
        assert_eq!(select(&tags(&["labview"]), &env), Selection::Run);
        assert_eq!(select(&tags(&[]), &env), Selection::NotLabview);
        assert_eq!(
            select(&tags(&["labview", "unsafe"]), &env),
            Selection::Unsafe
        );
        assert_eq!(
            select(&tags(&["labview", "pending-9"]), &env),
            Selection::Pending
        );
    }

    #[test]
    fn without_only_labview_every_safe_scenario_runs_and_labview_ones_need_the_flag() {
        let without_flag = Env::default();
        let with_flag = Env {
            test_labview: true,
            ..Env::default()
        };
        assert_eq!(select(&tags(&[]), &without_flag), Selection::Run);
        assert_eq!(
            select(&tags(&["labview"]), &without_flag),
            Selection::NeedsLabview
        );
        assert_eq!(select(&tags(&["labview"]), &with_flag), Selection::Run);
        assert_eq!(select(&tags(&["unsafe"]), &with_flag), Selection::Unsafe);
    }

    #[test]
    fn a_headless_labview_skips_the_interactive_scenarios_and_runs_the_rest() {
        let interactive = tags(&["labview", "interactive-labview"]);
        let env = |headless_labview| Env {
            test_labview: true,
            only_labview: true,
            headless_labview,
            ..Env::default()
        };
        assert_eq!(select(&interactive, &env(true)), Selection::Interactive);
        assert_eq!(select(&interactive, &env(false)), Selection::Run);
        assert_eq!(select(&tags(&["labview"]), &env(true)), Selection::Run);
    }

    #[test]
    fn a_global_labview_scenario_waits_for_a_run_that_has_the_global_labview() {
        let global = tags(&["labview", "global-labview"]);
        let env = |test_labview, global_busy| Env {
            test_labview,
            global_busy,
            ..Env::default()
        };
        assert_eq!(select(&global, &env(true, true)), Selection::GlobalBusy);
        assert_eq!(select(&global, &env(true, false)), Selection::Run);
        assert_eq!(
            select(&tags(&["labview"]), &env(true, true)),
            Selection::Run
        );
        assert_eq!(select(&global, &env(false, true)), Selection::NeedsLabview);
    }

    #[test]
    fn a_pilot_source_scenario_needs_the_checkout() {
        let pilot = tags(&["pilot-source"]);
        assert_eq!(select(&pilot, &Env::default()), Selection::NeedsPilotSource);
        let env = Env {
            pilot_a: true,
            ..Env::default()
        };
        assert_eq!(select(&pilot, &env), Selection::Run);
    }

    #[test]
    fn labview_is_asked_for_before_a_pilot_checkout() {
        let tagged = tags(&["labview", "pilot-source"]);
        assert_eq!(select(&tagged, &Env::default()), Selection::NeedsLabview);
        let labview = Env {
            test_labview: true,
            ..Env::default()
        };
        assert_eq!(select(&tagged, &labview), Selection::NeedsPilotSource);
    }

    #[test]
    fn the_not_run_line_text_names_the_variable_and_pilot() {
        assert_eq!(
            pilot_not_run_line(3, PILOT_SOURCE_TAG, PILOT_A_ENV_VAR, "A"),
            "3 @pilot-source scenarios not run (set LVPM_TEST_PILOT_A to a checkout of pilot A)"
        );
    }
}
