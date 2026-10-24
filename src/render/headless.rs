//! The wording of what a headless machine (`LV_RTE_HEADLESS`) changes: the
//! defaults lvpm picks there, and the phases it leaves out, in one place.

use super::{Renderer, Stream};
use lvpm_core::events::{Event, Phase};

pub(super) fn render(r: &mut Renderer, event: &Event) {
    match event {
        Event::HeadlessInstallationChosen { repo, labview } => r.line(
            Stream::Err,
            &format!(
                "headless (LV_RTE_HEADLESS): {} has a manifest and no venv — using {labview} itself",
                repo.display()
            ),
        ),
        Event::HeadlessImpliesLocked => r.line(
            Stream::Err,
            "note: headless mode implies --locked (LV_RTE_HEADLESS is set): lvpm.lock must \
             exist and be current; --no-locked re-locks instead",
        ),
        _ => unreachable!("routed here by Renderer::render: {event:?}"),
    }
}

/// What an install says about a phase that headless turned off, and what to do
/// if it was wanted. Says nothing of a phase headless never turns off.
pub(super) fn phase_skipped(r: &mut Renderer, phase: Phase) {
    let text = match phase {
        Phase::Relink => {
            "\nrelink: skipped — LabVIEW runs headless here (LV_RTE_HEADLESS); a compile or build\n      \
             resolves the links as it loads. --relink forces the pass, `lvpm relink --all` runs it later."
        }
        Phase::Hooks => {
            "\nhooks: skipped — LabVIEW runs headless here (LV_RTE_HEADLESS). Install hooks mostly add\n      \
             palettes, menus or markers for the IDE; a package whose hook does real setup needs --hooks."
        }
        Phase::Refresh => {
            "\npalettes and menus: not refreshed — LabVIEW runs headless here (`lvpm refresh` if wanted)"
        }
        _ => return,
    };
    r.line(Stream::Out, text);
}

#[cfg(test)]
mod tests {
    use super::super::tests::renderer;
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn a_headless_installation_choice_is_announced_on_stderr() {
        let (mut renderer, out, err) = renderer();
        let repo = PathBuf::from("proj");
        renderer.render(&Event::HeadlessInstallationChosen {
            repo: repo.clone(),
            labview: "LabVIEW 2026 (64-bit)  v26.3".into(),
        });
        assert_eq!(
            err.text(),
            format!(
                "headless (LV_RTE_HEADLESS): {} has a manifest and no venv — using LabVIEW 2026 (64-bit)  v26.3 itself\n",
                repo.display()
            )
        );
        assert_eq!(out.text(), "");
    }

    #[test]
    fn the_note_that_headless_implies_locked_is_one_line_on_stderr_and_names_the_way_out() {
        let (mut renderer, out, err) = renderer();
        renderer.render(&Event::HeadlessImpliesLocked);
        let note = err.text();
        assert_eq!(note.lines().count(), 1, "{note:?}");
        assert!(note.ends_with('\n'), "{note:?}");
        assert!(note.contains("headless mode implies --locked"), "{note}");
        assert!(note.contains("--no-locked"), "{note}");
        assert_eq!(out.text(), "");
    }
}
