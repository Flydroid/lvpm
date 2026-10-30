//! The wording of `lvpm init`: its events, what it prints when it is done, and
//! the errors it ends in.

use super::{Renderer, Stream};
use lvpm_core::errors::InitError;
use lvpm_core::events::{Event, Retarget};

pub(super) fn render(r: &mut Renderer, event: &Event) {
    match event {
        Event::InitManifestWritten {
            path,
            labview,
            installed,
            retarget,
        } => {
            r.line(Stream::Out, &format!("wrote {}", path.display()));
            if let Some(year) = labview {
                r.line(Stream::Out, &format!("  labview = \">={year}\""));
            }
            if let Some(retarget) = retarget {
                r.line(
                    Stream::Out,
                    &format!("  found LabVIEW {}", installed.join(", ")),
                );
                r.line(Stream::Out, "  to target another one:");
                if matches!(retarget, Retarget::Year(_)) {
                    r.line(Stream::Out, "    edit labview in lvpm.toml");
                }
                r.line(Stream::Out, "    lvpm venv remove");
                let chosen = match retarget {
                    Retarget::Year(year) => year.to_string(),
                    Retarget::Path(path) => format!("\"{}\"", path.display()),
                };
                r.line(
                    Stream::Out,
                    &format!("    lvpm venv create --labview-version {chosen}"),
                );
            }
        }
        Event::InitNoLabview => r.line(
            Stream::Err,
            "warning: no LabVIEW found; lvpm.toml has no `labview`. \
             Add labview = \">=YYYY\" before `lvpm lock`",
        ),
        _ => unreachable!("routed here by Renderer::render: {event:?}"),
    }
}

/// What `lvpm init` prints last: `labview` is the year the manifest states, if
/// it states one.
pub fn initialized(labview: Option<u32>) -> String {
    match labview {
        Some(_) => "next: lvpm sync\n".to_string(),
        None => "next: add labview = \">=YYYY\" to lvpm.toml, then lvpm sync\n".to_string(),
    }
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

    fn written(
        labview: Option<u32>,
        installed: &[&str],
        retarget: Option<Retarget>,
    ) -> (Event, PathBuf) {
        let path = PathBuf::from("proj").join("lvpm.toml");
        let event = Event::InitManifestWritten {
            path: path.clone(),
            labview,
            installed: installed.iter().map(|label| label.to_string()).collect(),
            retarget,
        };
        (event, path)
    }

    #[test]
    fn a_written_manifest_names_its_path_and_its_labview_on_stdout() {
        let (mut renderer, out, err) = renderer();
        let (event, path) = written(Some(2023), &["2026 (64-bit)"], None);
        renderer.render(&event);
        assert_eq!(
            out.text(),
            format!("wrote {}\n  labview = \">=2023\"\n", path.display())
        );
        assert_eq!(err.text(), "");
    }

    #[test]
    fn a_manifest_without_labview_has_no_labview_line() {
        let (mut renderer, out, _) = renderer();
        let (event, path) = written(None, &[], None);
        renderer.render(&event);
        assert_eq!(out.text(), format!("wrote {}\n", path.display()));
    }

    #[test]
    fn another_year_is_offered_with_the_edit_the_venv_remove_and_the_venv_create() {
        let (mut renderer, out, err) = renderer();
        let (event, path) = written(
            Some(2026),
            &["2026 (64-bit)", "2025 (64-bit)", "2024 (32-bit)"],
            Some(Retarget::Year(2025)),
        );
        renderer.render(&event);
        assert_eq!(
            out.text(),
            format!(
                "wrote {}\n  labview = \">=2026\"\n  \
                 found LabVIEW 2026 (64-bit), 2025 (64-bit), 2024 (32-bit)\n  \
                 to target another one:\n    edit labview in lvpm.toml\n    \
                 lvpm venv remove\n    lvpm venv create --labview-version 2025\n",
                path.display()
            )
        );
        assert_eq!(err.text(), "");
    }

    #[test]
    fn the_other_bitness_of_the_year_is_offered_by_its_quoted_path_with_no_edit_line() {
        let (mut renderer, out, _) = renderer();
        let other = PathBuf::from("LabVIEW 2026 32-bit");
        let (event, path) = written(
            Some(2026),
            &["2026 (64-bit)", "2026 (32-bit)"],
            Some(Retarget::Path(other.clone())),
        );
        renderer.render(&event);
        assert_eq!(
            out.text(),
            format!(
                "wrote {}\n  labview = \">=2026\"\n  \
                 found LabVIEW 2026 (64-bit), 2026 (32-bit)\n  \
                 to target another one:\n    lvpm venv remove\n    \
                 lvpm venv create --labview-version \"{}\"\n",
                path.display(),
                other.display()
            )
        );
    }

    #[test]
    fn no_labview_is_a_warning_on_stderr() {
        let (mut renderer, out, err) = renderer();
        renderer.render(&Event::InitNoLabview);
        assert_eq!(out.text(), "");
        assert_eq!(
            err.text(),
            "warning: no LabVIEW found; lvpm.toml has no `labview`. \
             Add labview = \">=YYYY\" before `lvpm lock`\n"
        );
    }

    #[test]
    fn init_ends_with_the_command_to_run_next() {
        assert_eq!(initialized(Some(2026)), "next: lvpm sync\n");
    }

    #[test]
    fn without_a_labview_the_next_step_is_to_add_one() {
        assert_eq!(
            initialized(None),
            "next: add labview = \">=YYYY\" to lvpm.toml, then lvpm sync\n"
        );
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
