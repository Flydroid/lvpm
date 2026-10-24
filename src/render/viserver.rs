//! The wording of the VI Server events.

use super::{Renderer, Stream};
use lvpm_core::events::Event;
use lvpm_core::ops::start::Started;

/// What `lvpm start` prints once the VI Server answers.
pub fn started(started: &Started) -> String {
    let already = match started.was_running {
        true => " (was already running)",
        false => "",
    };
    format!(
        "{} — VI Server answering on port {}{already}\n",
        started.target.label(),
        started.port
    )
}

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
    use lvpm_core::target::LvTarget;
    use std::path::PathBuf;

    #[test]
    fn a_started_labview_is_named_with_its_port_and_whether_it_was_running() {
        let target = LvTarget {
            version: 26.3,
            bitness: 64,
            path: PathBuf::from("LabVIEW 2026"),
        };
        let mut started_labview = Started {
            target,
            port: 3364,
            was_running: false,
        };
        assert_eq!(
            started(&started_labview),
            "LabVIEW 2026 (64-bit)  v26.3 — VI Server answering on port 3364\n"
        );
        started_labview.was_running = true;
        assert_eq!(
            started(&started_labview),
            "LabVIEW 2026 (64-bit)  v26.3 — VI Server answering on port 3364 (was already running)\n"
        );
    }

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
