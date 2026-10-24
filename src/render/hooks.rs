//! The wording of the hook events. A hook's line is part of whatever line its
//! operation has open: PreInstall extends its package's, the others stand alone.

use super::{Renderer, Stream};
use lvpm_core::events::{Event, HookKind};

/// Hook entries for printing. The manifest keeps them as `Name=path`, where
/// the path is wherever the VI sat on the package's build machine
/// (`<OS Boot Volume Root>\ci-builds\...`) — meaningless here, and long. Only
/// the name says anything to a reader.
pub(super) fn hook_names<I, S>(hooks: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    hooks
        .into_iter()
        .map(|h| {
            h.as_ref()
                .split_once('=')
                .map_or_else(|| h.as_ref().to_string(), |(n, _)| n.to_string())
        })
        .collect::<Vec<_>>()
        .join(", ")
}

pub(super) fn render(r: &mut Renderer, event: &Event) {
    match event {
        Event::PostInstallHooksStarted { count } => r.line(
            Stream::Out,
            &format!("\nrunning {count} post-install hook(s)"),
        ),
        // Only PostInstall waits on its own line; the others say nothing
        // until they have run.
        Event::HookStarting {
            package,
            hook: HookKind::PostInstall,
        } => r.begin(Stream::Out, &format!("  {package} ... ")),
        Event::HookStarting { .. } => {}
        Event::HookRan {
            hook: HookKind::PreInstall,
            took,
            ..
        } => r.append(
            Stream::Out,
            &format!("[pre-install ok, {:.1}s] ", took.as_secs_f64()),
        ),
        Event::HookRan {
            hook: HookKind::PostInstall,
            took,
            ..
        } => r.end(Stream::Out, &format!("ok ({:.1}s)", took.as_secs_f64())),
        Event::HookRan {
            hook: HookKind::PreUninstall,
            took,
            ..
        } => r.line(
            Stream::Out,
            &format!("pre-uninstall ok ({:.1}s)", took.as_secs_f64()),
        ),
        Event::HookRan {
            hook: HookKind::PostUninstall,
            took,
            ..
        } => r.line(
            Stream::Out,
            &format!("post-uninstall ok ({:.1}s)", took.as_secs_f64()),
        ),
        Event::HookFailed {
            hook: HookKind::PreInstall,
            error,
            ..
        } => r.append(Stream::Out, &format!("[pre-install FAILED: {error}] ")),
        Event::HookFailed {
            hook: HookKind::PostInstall,
            error,
            ..
        } => r.end(Stream::Out, &format!("FAILED: {error}")),
        Event::HookFailed {
            hook: HookKind::PreUninstall,
            error,
            ..
        } => r.line(
            Stream::Out,
            &format!("pre-uninstall FAILED: {error} — uninstalling anyway"),
        ),
        Event::HookFailed {
            hook: HookKind::PostUninstall,
            error,
            ..
        } => r.line(Stream::Out, &format!("post-uninstall FAILED: {error}")),
        // `lvpm run-hooks`: the same hook, on a line with no indent.
        Event::RerunHooksNothingToDo => r.line(
            Stream::Out,
            "nothing to do: no package with an extracted PostInstall hook",
        ),
        Event::RerunHookViGone { package, vi } => r.line(
            Stream::Out,
            &format!(
                "{package}: extracted hook is gone ({}) — reinstall the package",
                vi.display()
            ),
        ),
        Event::RerunHookStarting { package } => r.begin(Stream::Out, &format!("{package} ... ")),
        Event::RerunHookRan { took, .. } => {
            r.end(Stream::Out, &format!("ok ({:.1}s)", took.as_secs_f64()))
        }
        Event::RerunHookFailed { error, .. } => r.end(Stream::Out, &format!("FAILED: {error}")),
        _ => unreachable!("routed here by Renderer::render: {event:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::renderer;
    use super::*;
    use std::time::Duration;

    fn ran(hook: HookKind, millis: u64) -> Event {
        Event::HookRan {
            package: "pkg".into(),
            hook,
            took: Duration::from_millis(millis),
        }
    }

    fn failed(hook: HookKind) -> Event {
        Event::HookFailed {
            package: "pkg".into(),
            hook,
            error: "no VI Server".into(),
        }
    }

    #[test]
    fn a_post_install_hook_is_a_line_begun_before_it_runs_and_ended_after() {
        let (mut renderer, out, _) = renderer();
        renderer.render(&Event::PostInstallHooksStarted { count: 2 });
        renderer.render(&Event::HookStarting {
            package: "alpha".into(),
            hook: HookKind::PostInstall,
        });
        assert_eq!(out.text(), "\nrunning 2 post-install hook(s)\n  alpha ... ");
        renderer.render(&ran(HookKind::PostInstall, 2_000));
        renderer.render(&Event::HookStarting {
            package: "beta".into(),
            hook: HookKind::PostInstall,
        });
        renderer.render(&failed(HookKind::PostInstall));
        assert_eq!(
            out.text(),
            "\nrunning 2 post-install hook(s)\n  alpha ... ok (2.0s)\n  beta ... FAILED: no VI Server\n"
        );
    }

    #[test]
    fn a_pre_install_hook_extends_the_open_package_line() {
        let (mut renderer, out, _) = renderer();
        renderer.begin(Stream::Out, "+ alpha 2.0 ... ");
        renderer.render(&Event::HookStarting {
            package: "pkg".into(),
            hook: HookKind::PreInstall,
        });
        renderer.render(&failed(HookKind::PreInstall));
        assert_eq!(
            out.text(),
            "+ alpha 2.0 ... [pre-install FAILED: no VI Server] "
        );
    }

    #[test]
    fn the_uninstall_hooks_are_lines_of_their_own() {
        let (mut renderer, out, _) = renderer();
        renderer.render(&ran(HookKind::PreUninstall, 100));
        renderer.render(&failed(HookKind::PreUninstall));
        renderer.render(&ran(HookKind::PostUninstall, 300));
        renderer.render(&failed(HookKind::PostUninstall));
        assert_eq!(
            out.text(),
            "pre-uninstall ok (0.1s)\n\
             pre-uninstall FAILED: no VI Server — uninstalling anyway\n\
             post-uninstall ok (0.3s)\n\
             post-uninstall FAILED: no VI Server\n"
        );
    }

    #[test]
    fn a_rerun_hook_is_a_line_with_no_indent_begun_before_it_runs_and_ended_after() {
        let (mut renderer, out, _) = renderer();
        renderer.render(&Event::RerunHookStarting {
            package: "alpha".into(),
        });
        assert_eq!(out.text(), "alpha ... ");
        renderer.render(&Event::RerunHookRan {
            package: "alpha".into(),
            took: Duration::from_millis(2_000),
        });
        renderer.render(&Event::RerunHookStarting {
            package: "beta".into(),
        });
        renderer.render(&Event::RerunHookFailed {
            package: "beta".into(),
            error: "no VI Server".into(),
        });
        renderer.render(&Event::RerunHookViGone {
            package: "gamma".into(),
            vi: std::path::PathBuf::from("gone.vi"),
        });
        renderer.render(&Event::RerunHooksNothingToDo);
        assert_eq!(
            out.text(),
            "alpha ... ok (2.0s)\nbeta ... FAILED: no VI Server\n\
             gamma: extracted hook is gone (gone.vi) — reinstall the package\n\
             nothing to do: no package with an extracted PostInstall hook\n"
        );
    }

    #[test]
    fn a_hook_entry_is_shown_by_its_name_alone() {
        assert_eq!(
            hook_names(["PostInstall=<OS Boot Volume Root>\\ci\\x.vi", "Verify"]),
            "PostInstall, Verify"
        );
    }
}
