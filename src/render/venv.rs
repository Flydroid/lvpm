//! The wording of the venv events.

use super::{Renderer, Stream};
use lvpm_core::events::Event;
use lvpm_core::ops::venv_cmd::VenvStatus;
use lvpm_core::project;
use lvpm_core::venv::Venv;
use std::path::Path;

/// The lines about what a venv is bound to, shared by `create` and `status`.
fn binding(v: &Venv) -> String {
    format!(
        "  bound to       {}  ({})\n  VI Server port {}\n",
        v.target.label(),
        v.target.path.display(),
        v.port
    )
}

/// What `lvpm venv create` prints.
/// `manifest` is the one `--manifest` named, if any.
pub fn created(v: &Venv, manifest: Option<&Path>) -> String {
    let not_lvpm_toml =
        manifest.is_some_and(|m| m.file_name() != Some(project::FILE_NAME.as_ref()));
    let next = match not_lvpm_toml {
        // shortcut: lock and sync read only lvpm.toml; say how once another manifest can be locked.
        true => format!(
            "`lvpm launch` starts LabVIEW on it (its packages cannot be installed yet: `lvpm install` reads only {})",
            project::FILE_NAME
        ),
        false => format!(
            "`lvpm install` puts {}'s [dependencies] into it, `lvpm launch` starts LabVIEW on it",
            project::FILE_NAME
        ),
    };
    format!(
        "created {}\n{}\nnext: {next}\n",
        v.dir.display(),
        binding(v)
    )
}

/// What `lvpm venv status` prints. `searched_from` is where it looked for a venv.
pub fn status(status: Option<&VenvStatus>, searched_from: &Path) -> String {
    let Some(status) = status else {
        return format!(
            "no venv at or above {} — `lvpm venv create` makes one\n",
            searched_from.display()
        );
    };
    let v = &status.venv;
    let running = match (status.running, status.starting) {
        (true, _) => "running on this venv",
        (false, true) => "starting on this venv",
        (false, false) => "not running on this venv",
    };
    format!(
        "{}\n  project        {}\n{}  packages       {} installed, {} not yet relinked\n  LabVIEW        {running}\n",
        v.dir.display(),
        v.repo.display(),
        binding(v),
        status.installed,
        status.not_relinked,
    )
}

pub(super) fn render(r: &mut Renderer, event: &Event) {
    match event {
        Event::VenvManifestWritten { path } => {
            r.line(Stream::Err, &format!("wrote {}", path.display()))
        }
        Event::VenvRemoved { dir } => r.line(Stream::Out, &format!("removed {}", dir.display())),
        Event::VenvPortMoved { from, to } => r.line(
            Stream::Err,
            &format!("VI Server port {from} is reserved by Windows now; this venv uses port {to}"),
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

    fn venv() -> Venv {
        Venv {
            repo: PathBuf::from("proj"),
            dir: PathBuf::from("proj").join(".lv-venv"),
            target: LvTarget {
                version: 26.3,
                bitness: 64,
                path: PathBuf::from("LabVIEW 2026"),
            },
            port: 3412,
        }
    }

    #[test]
    fn a_created_venv_says_what_it_is_bound_to_and_what_to_do_next() {
        let v = venv();
        assert_eq!(
            created(&v, None),
            format!(
                "created {}\n  bound to       LabVIEW 2026 (64-bit)  v26.3  (LabVIEW 2026)\n  VI Server port 3412\n\nnext: `lvpm install` puts lvpm.toml's [dependencies] into it, `lvpm launch` starts LabVIEW on it\n",
                v.dir.display()
            )
        );
    }

    #[test]
    fn a_venv_made_from_a_vipm_toml_offers_no_install_step() {
        let text = created(&venv(), Some(Path::new("proj/vipm.toml")));
        assert!(text.contains("next: `lvpm launch` starts LabVIEW on it (its packages"));
        assert!(!text.contains("--manifest"));
    }

    #[test]
    fn a_status_counts_the_packages_and_says_whether_a_labview_runs_on_the_venv() {
        let v = venv();
        let found = VenvStatus {
            venv: v.clone(),
            installed: 3,
            not_relinked: 1,
            running: true,
            starting: false,
        };
        assert_eq!(
            status(Some(&found), Path::new("proj")),
            format!(
                "{}\n  project        proj\n  bound to       LabVIEW 2026 (64-bit)  v26.3  (LabVIEW 2026)\n  VI Server port 3412\n  packages       3 installed, 1 not yet relinked\n  LabVIEW        running on this venv\n",
                v.dir.display()
            )
        );
        assert_eq!(
            status(None, Path::new("proj")),
            "no venv at or above proj — `lvpm venv create` makes one\n"
        );
    }

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

        renderer.render(&Event::VenvPortMoved {
            from: 3412,
            to: 3413,
        });
        assert!(err.text().ends_with(
            "VI Server port 3412 is reserved by Windows now; this venv uses port 3413\n"
        ));
    }
}
