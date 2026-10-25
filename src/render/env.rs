//! The wording of the events that settling a destination reports.

use super::{Renderer, Stream};
use lvpm_core::events::Event;

pub(super) fn render(r: &mut Renderer, event: &Event) {
    match event {
        Event::VenvSelected { dir, labview } => r.line(
            Stream::Err,
            &format!("venv: {}  [{labview}]", dir.display()),
        ),
        _ => unreachable!("routed here by Renderer::render: {event:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::renderer;
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn a_selected_venv_is_announced_on_stderr() {
        let (mut renderer, out, err) = renderer();
        let dir = PathBuf::from("proj").join(".lv-venv");
        renderer.render(&Event::VenvSelected {
            dir: dir.clone(),
            labview: "LabVIEW 2026 (64-bit)  v26.3".into(),
        });
        assert_eq!(
            err.text(),
            format!("venv: {}  [LabVIEW 2026 (64-bit)  v26.3]\n", dir.display())
        );
        assert_eq!(out.text(), "");
    }
}
