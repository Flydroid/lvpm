//! The wording of the launch events.

use super::{Renderer, Stream};
use lvpm_core::events::Event;

pub(super) fn render(r: &mut Renderer, event: &Event) {
    match event {
        Event::VenvLabviewStarting { labview, port } => r.line(
            Stream::Err,
            &format!("starting {labview} on the venv and waiting for VI Server on port {port}..."),
        ),
        _ => unreachable!("routed here by Renderer::render: {event:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::renderer;
    use super::*;

    #[test]
    fn starting_a_venv_labview_is_announced_on_stderr() {
        let (mut renderer, out, err) = renderer();
        renderer.render(&Event::VenvLabviewStarting {
            labview: "LabVIEW 2026 (64-bit)".into(),
            port: 3412,
        });
        assert_eq!(
            err.text(),
            "starting LabVIEW 2026 (64-bit) on the venv and waiting for VI Server on port 3412...\n"
        );
        assert_eq!(out.text(), "");
    }
}
