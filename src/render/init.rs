//! The wording of `lvpm init`: its events, what it prints when it is done, and
//! the errors it ends in.

use super::{Renderer, Stream};
use lvpm_core::errors::InitError;
use lvpm_core::events::{Event, Retarget};
use lvpm_core::git::GitOutcome;
use lvpm_core::ops::init::Initialized;

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
        Event::InitGit { outcome } => git(r, outcome),
        _ => unreachable!("routed here by Renderer::render: {event:?}"),
    }
}

fn git(r: &mut Renderer, outcome: &GitOutcome) {
    match outcome {
        GitOutcome::Initialized => {
            r.line(Stream::Out, "initialized a git repository");
            r.line(Stream::Out, "  add a remote:  git remote add origin <url>");
            r.line(
                Stream::Out,
                "  first push:    git add -A && git commit -m \"lvpm init\" && git push -u origin HEAD",
            );
        }
        GitOutcome::InsideWorkTree { top } => r.line(
            Stream::Out,
            &format!(
                "already inside a git repository at {}; left it as it is",
                top.display()
            ),
        ),
        GitOutcome::GitMissing => r.line(
            Stream::Err,
            "warning: git not found on PATH; no repository made",
        ),
        GitOutcome::Failed { why } => r.line(
            Stream::Err,
            &format!("warning: git init failed: {why}; no repository made"),
        ),
    }
}

/// What `lvpm init` prints last: the venv it made or why not, then what to do next.
pub fn initialized(done: &Initialized) -> String {
    let venv = match &done.venv {
        Ok(v) => format!(
            "created {}\n  bound to       {}\n",
            v.dir.display(),
            v.target.label()
        ),
        Err(skip) => format!("no venv: {}\n  later: lvpm venv create\n", skip.why),
    };
    let next = match done.labview {
        Some(_) => "next: lvpm sync\n",
        None => "next: add labview = \">=YYYY\" to lvpm.toml, then lvpm sync\n",
    };
    format!("{venv}{next}")
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
        InitError::BadLabview { .. } => format!("{error}; nothing was written"),
        InitError::Other(cause) => format!("{cause:#}"),
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::renderer;
    use super::*;
    use lvpm_core::ops::init::VenvSkip;
    use lvpm_core::target::LvTarget;
    use lvpm_core::venv::Venv;
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
    fn a_new_repository_says_how_to_add_a_remote_and_push_on_stdout() {
        let (mut renderer, out, err) = renderer();
        renderer.render(&Event::InitGit {
            outcome: GitOutcome::Initialized,
        });
        assert_eq!(
            out.text(),
            "initialized a git repository\n  \
             add a remote:  git remote add origin <url>\n  \
             first push:    git add -A && git commit -m \"lvpm init\" && git push -u origin HEAD\n"
        );
        assert_eq!(err.text(), "");
    }

    #[test]
    fn a_repository_that_was_there_is_named_and_left_alone_on_stdout() {
        let (mut renderer, out, err) = renderer();
        let top = PathBuf::from("repo");
        renderer.render(&Event::InitGit {
            outcome: GitOutcome::InsideWorkTree { top: top.clone() },
        });
        assert_eq!(
            out.text(),
            format!(
                "already inside a git repository at {}; left it as it is\n",
                top.display()
            )
        );
        assert_eq!(err.text(), "");
    }

    #[test]
    fn git_missing_and_git_failing_are_warnings_on_stderr() {
        let (mut renderer, out, err) = renderer();
        renderer.render(&Event::InitGit {
            outcome: GitOutcome::GitMissing,
        });
        renderer.render(&Event::InitGit {
            outcome: GitOutcome::Failed {
                why: "fatal: permission denied".into(),
            },
        });
        assert_eq!(out.text(), "");
        assert_eq!(
            err.text(),
            "warning: git not found on PATH; no repository made\n\
             warning: git init failed: fatal: permission denied; no repository made\n"
        );
    }

    fn done(labview: Option<u32>, venv: Result<Venv, VenvSkip>) -> Initialized {
        Initialized {
            manifest: PathBuf::from("proj").join("lvpm.toml"),
            labview,
            venv,
        }
    }

    fn skipped(why: &str) -> Result<Venv, VenvSkip> {
        Err(VenvSkip { why: why.into() })
    }

    fn a_venv() -> Venv {
        let repo = PathBuf::from("proj");
        Venv {
            dir: repo.join(".lv-venv"),
            repo,
            target: LvTarget {
                version: 26.3,
                bitness: 64,
                path: PathBuf::from("LabVIEW 2026"),
            },
            port: 3412,
        }
    }

    #[test]
    fn a_created_venv_is_named_with_what_it_is_bound_to_and_the_next_command() {
        let venv = a_venv();
        assert_eq!(
            initialized(&done(Some(2026), Ok(venv.clone()))),
            format!(
                "created {}\n  bound to       LabVIEW 2026 (64-bit)  v26.3\nnext: lvpm sync\n",
                venv.dir.display()
            )
        );
    }

    #[test]
    fn a_skipped_venv_gives_its_reason_and_the_bare_command_for_later() {
        assert_eq!(
            initialized(&done(Some(2025), skipped("LabVIEW 2025 is not installed"))),
            "no venv: LabVIEW 2025 is not installed\n  later: lvpm venv create\nnext: lvpm sync\n"
        );
    }

    #[test]
    fn without_a_labview_the_next_step_is_to_add_one() {
        assert_eq!(
            initialized(&done(None, skipped("no LabVIEW found"))),
            "no venv: no LabVIEW found\n  later: lvpm venv create\n\
             next: add labview = \">=YYYY\" to lvpm.toml, then lvpm sync\n"
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
        assert_eq!(
            error(&InitError::BadLabview {
                value: "26.3".into()
            }),
            "--labview-version 26.3 is not a LabVIEW year (expected e.g. 2025); nothing was written"
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
