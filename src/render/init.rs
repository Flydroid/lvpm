//! The wording of `lvpm init`: its events, what it prints when it is done, and
//! the errors it ends in.

use super::{Renderer, Stream};
use lvpm_core::errors::InitError;
use lvpm_core::events::{Event, Retarget, SourceKind};
use lvpm_core::git::GitOutcome;
use lvpm_core::ops::init::Initialized;

pub(super) fn render(r: &mut Renderer, event: &Event) {
    match event {
        Event::InitManifestWritten {
            path,
            labview,
            installed,
            retarget,
            source_labview,
            source_kind,
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
            if let (Some(kind), Some(source)) = (source_kind, source_labview)
                && *labview != Some(*source)
            {
                let file = match kind {
                    SourceKind::Vipc => ".vipc",
                    SourceKind::VipmToml => "vipm.toml",
                };
                let says = labview
                    .map(|year| format!("; lvpm.toml says >={year}"))
                    .unwrap_or_default();
                r.line(
                    Stream::Out,
                    &format!("  the {file} targets LabVIEW {source}{says}"),
                );
            }
        }
        Event::InitNoLabview => r.line(
            Stream::Err,
            "warning: no LabVIEW found; lvpm.toml has no `labview`. \
             Add labview = \">=YYYY\" before `lvpm lock`",
        ),
        Event::InitDependenciesAdded {
            from,
            added,
            skipped,
        } => {
            let noun = if added.len() == 1 {
                "dependency"
            } else {
                "dependencies"
            };
            r.line(
                Stream::Out,
                &format!("added {} {noun} from {from}", added.len()),
            );
            for dependency in added {
                r.line(
                    Stream::Out,
                    // Names and constraints come from a file the user was handed.
                    &super::printable(format!("  {} {}", dependency.name, dependency.constraint)),
                );
            }
            if *skipped > 0 {
                let noun = if *skipped == 1 { "package" } else { "packages" };
                r.line(
                    Stream::Out,
                    &format!("  skipped {skipped} {noun} VIPM marked as a dependency of another"),
                );
            }
        }
        Event::InitPackageImported {
            name,
            version,
            from,
        } => r.line(
            Stream::Out,
            &format!("  imported {name} {version} from {from}"),
        ),
        Event::InitPackageNotEmbedded {
            name,
            version,
            from,
        } => r.line(
            Stream::Err,
            &format!("warning: {name} {version} is marked Included but is not inside {from}"),
        ),
        Event::InitPackageImportFailed { name, version, why } => r.line(
            Stream::Err,
            &format!(
                "warning: could not import {name} {version} into the cache: {why}; \
                 add it later with lvpm cache add <archive>"
            ),
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
            &super::printable(format!(
                "warning: git init failed: {why}; no repository made"
            )),
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
    // `lvpm sync` refuses a folder with an lvpm.toml and no venv.
    let next = match (done.labview, &done.venv) {
        (None, _) => "next: add labview = \">=YYYY\" to lvpm.toml, then lvpm venv create\n",
        (Some(_), Ok(_)) => "next: lvpm sync\n",
        (Some(_), Err(_)) => "next: lvpm venv create\n",
    };
    format!("{venv}{next}")
}

/// What `lvpm init` prints when it fails: what happened, and that nothing was
/// written when that is so.
pub fn error(error: &InitError) -> String {
    match error {
        InitError::ProjectMissing { .. }
        | InitError::BadLabview { .. }
        | InitError::BadSource { .. } => format!("{error}; nothing was written"),
        InitError::DragonFile { .. } => error.to_string(),
        InitError::ManifestExists { .. } => format!(
            "{error}; lvpm init does not overwrite it\n  \
             this folder is already an lvpm project: lvpm add puts packages in it, \
             lvpm venv create makes its venv"
        ),
        InitError::Other(cause) => format!("{cause:#}"),
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::renderer;
    use super::*;
    use lvpm_core::events::AddedDependency;
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
            source_labview: None,
            source_kind: None,
        };
        (event, path)
    }

    /// `labview` as written, and a file of `kind` that targets `source`.
    fn written_from(kind: SourceKind, labview: Option<u32>, source: u32) -> Event {
        Event::InitManifestWritten {
            path: PathBuf::from("proj").join("lvpm.toml"),
            labview,
            installed: Vec::new(),
            retarget: None,
            source_labview: Some(source),
            source_kind: Some(kind),
        }
    }

    fn written_from_vipc(labview: Option<u32>, source: u32) -> Event {
        written_from(SourceKind::Vipc, labview, source)
    }

    fn added(deps: &[(&str, &str)], skipped: usize) -> Event {
        Event::InitDependenciesAdded {
            from: "p.vipc".into(),
            added: deps
                .iter()
                .map(|(name, constraint)| AddedDependency {
                    name: name.to_string(),
                    constraint: constraint.to_string(),
                })
                .collect(),
            skipped,
        }
    }

    #[test]
    fn a_vipc_for_another_labview_is_reported_with_what_lvpm_toml_says() {
        let (mut renderer, out, err) = renderer();
        renderer.render(&written_from_vipc(Some(2023), 2013));
        assert_eq!(
            out.text(),
            format!(
                "wrote {}\n  labview = \">=2023\"\n  \
                 the .vipc targets LabVIEW 2013; lvpm.toml says >=2023\n",
                PathBuf::from("proj").join("lvpm.toml").display()
            )
        );
        assert_eq!(err.text(), "");
    }

    #[test]
    fn a_vipm_toml_for_another_labview_is_reported_as_the_vipm_toml() {
        let (mut renderer, out, _) = renderer();
        renderer.render(&written_from(SourceKind::VipmToml, Some(2023), 2021));
        assert_eq!(
            out.text().lines().last(),
            Some("  the vipm.toml targets LabVIEW 2021; lvpm.toml says >=2023")
        );
    }

    #[test]
    fn a_vipc_for_the_chosen_labview_is_not_reported() {
        let (mut renderer, out, _) = renderer();
        renderer.render(&written_from_vipc(Some(2023), 2023));
        assert!(!out.text().contains("targets"), "{}", out.text());
    }

    #[test]
    fn a_vipc_with_no_labview_chosen_is_reported_with_no_lvpm_toml_part() {
        let (mut renderer, out, _) = renderer();
        renderer.render(&written_from_vipc(None, 2013));
        assert_eq!(
            out.text().lines().last(),
            Some("  the .vipc targets LabVIEW 2013")
        );
    }

    #[test]
    fn the_added_dependencies_are_counted_listed_and_the_skipped_ones_counted() {
        let (mut renderer, out, err) = renderer();
        renderer.render(&added(&[("a_lib", "1.0.0.1"), ("b_lib", ">=2.0.0.2")], 3));
        assert_eq!(
            out.text(),
            "added 2 dependencies from p.vipc\n  a_lib 1.0.0.1\n  b_lib >=2.0.0.2\n  \
             skipped 3 packages VIPM marked as a dependency of another\n"
        );
        assert_eq!(err.text(), "");
    }

    #[test]
    fn one_dependency_and_one_skip_are_singular() {
        let (mut renderer, out, _) = renderer();
        renderer.render(&added(&[("a_lib", "1.0.0.1")], 1));
        assert_eq!(
            out.text(),
            "added 1 dependency from p.vipc\n  a_lib 1.0.0.1\n  \
             skipped 1 package VIPM marked as a dependency of another\n"
        );
    }

    #[test]
    fn no_dependencies_is_plural_and_says_nothing_about_skips_when_none_were() {
        let (mut renderer, out, _) = renderer();
        renderer.render(&added(&[], 0));
        assert_eq!(out.text(), "added 0 dependencies from p.vipc\n");
    }

    #[test]
    fn an_imported_package_is_a_line_on_stdout_and_a_missing_one_a_warning_on_stderr() {
        let (mut renderer, out, err) = renderer();
        renderer.render(&Event::InitPackageImported {
            name: "demo_lib".into(),
            version: "1.0.0".into(),
            from: "p.vipc".into(),
        });
        assert_eq!(out.text(), "  imported demo_lib 1.0.0 from p.vipc\n");
        assert_eq!(err.text(), "");

        renderer.render(&Event::InitPackageNotEmbedded {
            name: "demo_lib".into(),
            version: "1.0.0".into(),
            from: "p.vipc".into(),
        });
        assert_eq!(out.text(), "  imported demo_lib 1.0.0 from p.vipc\n");
        assert_eq!(
            err.text(),
            "warning: demo_lib 1.0.0 is marked Included but is not inside p.vipc\n"
        );
    }

    #[test]
    fn a_failed_import_is_a_warning_with_the_reason_and_how_to_add_it_later() {
        let (mut renderer, out, err) = renderer();
        renderer.render(&Event::InitPackageImportFailed {
            name: "demo_lib".into(),
            version: "1.0.0".into(),
            why: "disk full".into(),
        });
        assert_eq!(out.text(), "");
        assert_eq!(
            err.text(),
            "warning: could not import demo_lib 1.0.0 into the cache: disk full; \
             add it later with lvpm cache add <archive>\n"
        );
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
            "no venv: LabVIEW 2025 is not installed\n  later: lvpm venv create\nnext: lvpm venv create\n"
        );
    }

    #[test]
    fn without_a_labview_the_next_step_is_to_add_one() {
        assert_eq!(
            initialized(&done(None, skipped("no LabVIEW found"))),
            "no venv: no LabVIEW found\n  later: lvpm venv create\n\
             next: add labview = \">=YYYY\" to lvpm.toml, then lvpm venv create\n"
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
    fn a_bad_from_file_says_nothing_was_written_and_a_dragon_file_is_not_supported() {
        assert_eq!(
            error(&InitError::BadSource {
                why: "p.vipc: embedded demo_lib-1.0.0.vip is not a package: not a zip".into()
            }),
            "p.vipc: embedded demo_lib-1.0.0.vip is not a package: not a zip; \
             nothing was written"
        );
        assert_eq!(
            error(&InitError::BadSource {
                why: "p.vipc: not a zip file".into()
            }),
            "p.vipc: not a zip file; nothing was written"
        );
        assert_eq!(
            error(&InitError::DragonFile {
                file: "p.dragon".into()
            }),
            "p.dragon: .dragon files are not supported and not planned \
             (VIPM discontinued the format); export a .vipc from VIPM instead"
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
