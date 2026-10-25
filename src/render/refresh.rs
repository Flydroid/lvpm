//! The wording of the palette and menu refresh events.

use super::{Renderer, Stream};
use lvpm_core::events::Event;

pub(super) fn render(r: &mut Renderer, event: &Event) {
    match event {
        Event::RefreshStarted => r.line(Stream::Out, "\nrefreshing palettes and menus"),
        Event::RefreshStepDone { what, took } => r.line(
            Stream::Out,
            &format!("  {what:<9} ok ({:.1}s)", took.as_secs_f64()),
        ),
        // A restart does what the refresh would have: say so, since
        // otherwise the package looks broken rather than merely unlisted.
        Event::RefreshStepFailed { what, error } => r.line(
            Stream::Out,
            &format!("  {what:<9} FAILED: {error}\n      restart LabVIEW to pick these up"),
        ),
        Event::RefreshUnavailable { error } => {
            r.line(Stream::Out, &format!("  SKIPPED: {error}"));
            r.line(
                Stream::Out,
                "  the files are installed; `lvpm refresh` retries just this step",
            );
        }
        _ => unreachable!("routed here by Renderer::render: {event:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::renderer;
    use super::*;
    use std::time::Duration;

    #[test]
    fn each_step_is_a_padded_line_and_a_failure_says_to_restart() {
        let (mut renderer, out, err) = renderer();
        renderer.render(&Event::RefreshStarted);
        renderer.render(&Event::RefreshStepDone {
            what: "palettes".into(),
            took: Duration::from_millis(2_400),
        });
        renderer.render(&Event::RefreshStepFailed {
            what: "menus".into(),
            error: "no such VI".into(),
        });
        assert_eq!(
            out.text(),
            "\nrefreshing palettes and menus\n  palettes  ok (2.4s)\n  menus     FAILED: no such VI\n      restart LabVIEW to pick these up\n"
        );
        assert_eq!(err.text(), "");
    }

    #[test]
    fn a_refresh_that_could_not_start_says_the_files_are_installed() {
        let (mut renderer, out, _) = renderer();
        renderer.render(&Event::RefreshUnavailable {
            error: "VI Server is off".into(),
        });
        assert_eq!(
            out.text(),
            "  SKIPPED: VI Server is off\n  the files are installed; `lvpm refresh` retries just this step\n"
        );
    }
}
