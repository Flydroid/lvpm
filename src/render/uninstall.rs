//! The wording of the uninstall events.

use super::hooks::hook_names;
use super::{Renderer, Stream};
use lvpm_core::events::{Event, UnrunReason};

pub(super) fn render(r: &mut Renderer, event: &Event) {
    match event {
        Event::PackageRemoved {
            name,
            version,
            files,
        } => r.line(
            Stream::Out,
            &format!("removed {name} {version} ({files} files)"),
        ),
        Event::UninstallFailed { name, error } => {
            r.line(Stream::Out, &format!("uninstall {name} FAILED: {error}"))
        }
        Event::PostUninstallNotStaged { error } => r.line(
            Stream::Out,
            &format!("note: cannot stage PostUninstall.vi ({error}) — not running it"),
        ),
        Event::UninstallHooksNotRun { hooks, reason } => {
            let why = match reason {
                UnrunReason::InstalledBeforeUninstallHooks => {
                    "installed before uninstall hooks existed"
                }
                UnrunReason::VenvRunsNoHooks => "hooks do not run in a venv",
            };
            r.line(
                Stream::Out,
                &format!("note: declared but not run ({why}): {}", hook_names(hooks)),
            );
        }
        _ => unreachable!("routed here by Renderer::render: {event:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::renderer;
    use super::*;

    #[test]
    fn a_removal_and_a_failed_sweep_member_are_lines_on_stdout() {
        let (mut renderer, out, err) = renderer();
        renderer.render(&Event::PackageRemoved {
            name: "beta".into(),
            version: "1.0".into(),
            files: 1,
        });
        renderer.render(&Event::UninstallFailed {
            name: "alpha".into(),
            error: "removing the file".into(),
        });
        assert_eq!(
            out.text(),
            "removed beta 1.0 (1 files)\nuninstall alpha FAILED: removing the file\n"
        );
        assert_eq!(err.text(), "");
    }

    #[test]
    fn an_unrun_hook_is_named_with_the_reason() {
        let (mut renderer, out, _) = renderer();
        renderer.render(&Event::UninstallHooksNotRun {
            hooks: vec!["PreUninstall=a.vi".into(), "PostUninstall=b.vi".into()],
            reason: UnrunReason::VenvRunsNoHooks,
        });
        renderer.render(&Event::UninstallHooksNotRun {
            hooks: vec!["PreUninstall=a.vi".into()],
            reason: UnrunReason::InstalledBeforeUninstallHooks,
        });
        renderer.render(&Event::PostUninstallNotStaged {
            error: "access denied".into(),
        });
        assert_eq!(
            out.text(),
            "note: declared but not run (hooks do not run in a venv): PreUninstall, PostUninstall\n\
             note: declared but not run (installed before uninstall hooks existed): PreUninstall\n\
             note: cannot stage PostUninstall.vi (access denied) — not running it\n"
        );
    }
}
