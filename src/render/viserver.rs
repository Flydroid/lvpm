//! The wording of the VI Server events.

use super::{Renderer, Stream};
use lvpm_core::events::Event;

pub(super) fn render(r: &mut Renderer, event: &Event) {
    match event {
        Event::LabviewStarting { labview, port } => r.line(
            Stream::Err,
            &format!("starting {labview} and waiting for VI Server on port {port}..."),
        ),
        _ => unreachable!("routed here by Renderer::render: {event:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::renderer;
    use super::*;

    #[test]
    fn starting_an_installations_labview_is_announced_on_stderr() {
        let (mut renderer, out, err) = renderer();
        renderer.render(&Event::LabviewStarting {
            labview: "LabVIEW 2026 (64-bit)".into(),
            port: 3364,
        });
        assert_eq!(
            err.text(),
            "starting LabVIEW 2026 (64-bit) and waiting for VI Server on port 3364...\n"
        );
        assert_eq!(out.text(), "");
    }
}
