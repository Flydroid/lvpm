//! The wording of the relink events.

use super::{Renderer, Stream};
use lvpm_core::events::Event;

pub(super) fn render(r: &mut Renderer, event: &Event) {
    match event {
        Event::RelinkDoneNotClearable { indicator, error } => r.line(
            Stream::Err,
            &format!(
                "      note: cannot clear {indicator} ({error}) — relying on the VI to clear it"
            ),
        ),
        Event::RelinkProgress { elapsed, value } => r.line(
            Stream::Out,
            &format!("        {:>6.0}s  {value}", elapsed.as_secs_f64()),
        ),
        Event::RelinkProgressUnreadable { indicator, error } => r.line(
            Stream::Err,
            &format!("      note: cannot read {indicator} ({error}) — not reporting progress"),
        ),
        _ => unreachable!("routed here by Renderer::render: {event:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::renderer;
    use super::*;
    use std::time::Duration;

    #[test]
    fn progress_is_a_right_aligned_second_count_and_the_value_on_stdout() {
        let (mut renderer, out, err) = renderer();
        renderer.render(&Event::RelinkProgress {
            elapsed: Duration::from_secs(12),
            value: "Walking vi.lib".into(),
        });
        assert_eq!(out.text(), "            12s  Walking vi.lib\n");
        assert_eq!(err.text(), "");
    }

    #[test]
    fn the_notes_go_to_stderr() {
        let (mut renderer, out, err) = renderer();
        renderer.render(&Event::RelinkDoneNotClearable {
            indicator: "Done".into(),
            error: "error 1055".into(),
        });
        renderer.render(&Event::RelinkProgressUnreadable {
            indicator: "Status".into(),
            error: "no such control".into(),
        });
        assert_eq!(
            err.text(),
            "      note: cannot clear Done (error 1055) — relying on the VI to clear it\n\
             \x20     note: cannot read Status (no such control) — not reporting progress\n"
        );
        assert_eq!(out.text(), "");
    }
}
