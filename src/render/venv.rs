//! The wording of the venv events.

use super::{Renderer, Stream};
use lvpm_core::events::Event;

pub(super) fn render(r: &mut Renderer, event: &Event) {
    match event {
        Event::VenvManifestWritten { path } => {
            r.line(Stream::Err, &format!("wrote {}", path.display()))
        }
        Event::VenvRemoved { dir } => r.line(Stream::Out, &format!("removed {}", dir.display())),
        _ => unreachable!("routed here by Renderer::render: {event:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::renderer;
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn a_written_manifest_is_reported_on_stderr_and_a_removal_on_stdout() {
        let (mut renderer, out, err) = renderer();
        let path = PathBuf::from("proj").join("lvpm.toml");
        renderer.render(&Event::VenvManifestWritten { path: path.clone() });
        assert_eq!(err.text(), format!("wrote {}\n", path.display()));
        assert_eq!(out.text(), "");

        let dir = PathBuf::from("proj").join(".lv-venv");
        renderer.render(&Event::VenvRemoved { dir: dir.clone() });
        assert_eq!(out.text(), format!("removed {}\n", dir.display()));
    }
}
