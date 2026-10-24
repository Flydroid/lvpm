//! The wording of the launch events.

use super::{Renderer, Stream};
use lvpm_core::events::Event;
use lvpm_core::ops::launch_cmd::Launched;

/// What `lvpm launch` prints once LabVIEW is started.
pub fn launched(launched: &Launched) -> String {
    let v = &launched.venv;
    let mut text = format!(
        "started {} on {}\n  ini            {}\n  VI Server port {}\n",
        v.target.label(),
        v.dir.display(),
        launched.ini.display(),
        v.port
    );
    if let Some(p) = &launched.lvproj {
        text.push_str(&format!("  opening        {}\n", p.display()));
    }
    text
}

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
    use lvpm_core::target::LvTarget;
    use lvpm_core::venv::Venv;
    use std::path::PathBuf;

    #[test]
    fn a_launch_names_the_venv_its_ini_and_port_and_the_project_it_opens() {
        let dir = PathBuf::from("proj").join(".lv-venv");
        let mut done = Launched {
            venv: Venv {
                repo: PathBuf::from("proj"),
                dir: dir.clone(),
                target: LvTarget {
                    version: 26.3,
                    bitness: 64,
                    path: PathBuf::from("LabVIEW 2026"),
                },
                port: 3412,
            },
            ini: dir.join("labview.ini"),
            lvproj: None,
        };
        let head = format!(
            "started LabVIEW 2026 (64-bit)  v26.3 on {}\n  ini            {}\n  VI Server port 3412\n",
            dir.display(),
            dir.join("labview.ini").display()
        );
        assert_eq!(launched(&done), head);
        done.lvproj = Some(PathBuf::from("proj").join("a.lvproj"));
        assert_eq!(
            launched(&done),
            format!(
                "{head}  opening        {}\n",
                PathBuf::from("proj").join("a.lvproj").display()
            )
        );
    }

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
