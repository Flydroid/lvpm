//! The wording of `lvpm init`: its events, what it prints when it is done, and
//! the errors it ends in.

use super::{Renderer, Stream};
use lvpm_core::errors::InitError;
use lvpm_core::events::Event;

pub(super) fn render(r: &mut Renderer, event: &Event) {
    match event {
        Event::InitManifestWritten { path, labview } => {
            r.line(Stream::Out, &format!("wrote {}", path.display()));
            if let Some(year) = labview {
                r.line(Stream::Out, &format!("  labview = \">={year}\""));
            }
        }
        _ => unreachable!("routed here by Renderer::render: {event:?}"),
    }
}

/// What `lvpm init` prints last.
pub fn initialized() -> String {
    "next: lvpm sync\n".to_string()
}

/// What `lvpm init` prints when it fails: what happened, and that nothing was
/// written when that is so.
pub fn error(error: &InitError) -> String {
    match error {
        InitError::ProjectMissing { .. } => format!("{error}; nothing was written"),
        InitError::ManifestExists { .. } => format!(
            "{error}; lvpm init does not overwrite it\n  \
             this folder is already an lvpm project: lvpm add puts packages in it, \
             lvpm venv create makes its venv"
        ),
        InitError::BadLabview { value, why } => {
            format!("--labview-version {value}: {why}; nothing was written")
        }
        InitError::Other(cause) => format!("{cause:#}"),
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::renderer;
    use super::*;
    use lvpm_core::errors::LockError;
    use std::path::PathBuf;

    #[test]
    fn a_written_manifest_names_its_path_and_its_labview_on_stdout() {
        let (mut renderer, out, err) = renderer();
        let path = PathBuf::from("proj").join("lvpm.toml");
        renderer.render(&Event::InitManifestWritten {
            path: path.clone(),
            labview: Some(2023),
        });
        assert_eq!(
            out.text(),
            format!("wrote {}\n  labview = \">=2023\"\n", path.display())
        );
        assert_eq!(err.text(), "");
    }

    #[test]
    fn a_manifest_without_labview_has_no_labview_line() {
        let (mut renderer, out, _) = renderer();
        let path = PathBuf::from("proj").join("lvpm.toml");
        renderer.render(&Event::InitManifestWritten {
            path: path.clone(),
            labview: None,
        });
        assert_eq!(out.text(), format!("wrote {}\n", path.display()));
    }

    #[test]
    fn init_ends_with_the_command_to_run_next() {
        assert_eq!(initialized(), "next: lvpm sync\n");
    }

    #[test]
    fn an_existing_manifest_is_named_and_the_user_is_told_what_the_folder_already_is() {
        let path = PathBuf::from("proj").join("lvpm.toml");
        assert_eq!(
            error(&InitError::ManifestExists { path: path.clone() }),
            format!(
                "{} already exists; lvpm init does not overwrite it\n  \
                 this folder is already an lvpm project: lvpm add puts packages in it, \
                 lvpm venv create makes its venv",
                path.display()
            )
        );
    }

    #[test]
    fn a_missing_project_folder_and_a_bad_flag_say_nothing_was_written() {
        let dir = PathBuf::from("Missing");
        assert_eq!(
            error(&InitError::ProjectMissing { dir }),
            "Missing does not exist; nothing was written"
        );
        let why = LockError::LabviewNotAYear {
            value: "26.3".into(),
        };
        assert_eq!(
            error(&InitError::BadLabview {
                value: "26.3".into(),
                why
            }),
            "--labview-version 26.3: labview = \"26.3\" in lvpm.toml is not a minimum LabVIEW year; nothing was written"
        );
    }

    #[test]
    fn any_other_failure_is_its_own_text_with_its_causes() {
        let cause = anyhow::anyhow!("denied").context("writing proj/lvpm.toml");
        assert_eq!(
            error(&InitError::Other(cause)),
            "writing proj/lvpm.toml: denied"
        );
    }
}
