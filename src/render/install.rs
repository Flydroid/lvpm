//! The wording of the install's events: what it reads, what it resolved, what
//! each package did, and the phases that did not run.

use super::hooks::hook_names;
use super::{Renderer, Stream};
use lvpm_core::events::{Event, Phase, PlannedFile, SkipReason};

/// How many of a package's planned writes a dry run lists before "and N more".
const PREVIEW_LIMIT: usize = 6;

fn plural(count: usize, one: &'static str, many: &'static str) -> &'static str {
    if count == 1 { one } else { many }
}

pub(super) fn render(r: &mut Renderer, event: &Event) {
    match event {
        Event::ManifestInUse {
            path,
            name,
            dependencies,
            labview,
        } => {
            let named = name.as_deref().map(|n| format!(" ({n})")).unwrap_or_default();
            r.line(
                Stream::Err,
                &format!(
                    "manifest: {}{named} — {dependencies} dependenc{}",
                    path.display(),
                    plural(*dependencies, "y", "ies")
                ),
            );
            if let Some(v) = labview {
                r.line(
                    Stream::Err,
                    &format!("      manifest says labview = {v:?} (minimum)"),
                );
            }
        }
        Event::InstallTarget {
            labview: Some(label),
            path,
        } => r.line(
            Stream::Err,
            &format!("target: {label}  ({})", path.display()),
        ),
        Event::InstallTarget {
            labview: None,
            path,
        } => r.line(
            Stream::Err,
            &format!("target: scratch tree at {}", path.display()),
        ),
        Event::IndexesLoading => r.line(Stream::Err, "loading indexes..."),
        Event::IndexesLoaded { versions } => r.line(
            Stream::Err,
            &format!("  {versions} package versions known"),
        ),
        Event::DependencySatisfied { name, version } => r.line(
            Stream::Err,
            &format!("  = {name} {version} already installed, satisfies dependency"),
        ),
        Event::DependencyUnresolved { name } => r.line(
            Stream::Err,
            &format!("  ! dependency {name} unresolved, skipping"),
        ),
        Event::Resolved { packages } => {
            r.line(
                Stream::Out,
                &format!(
                    "\nresolved {} package{}:",
                    packages.len(),
                    plural(packages.len(), "", "s")
                ),
            );
            for package in packages {
                r.line(
                    Stream::Out,
                    &format!("  {} {}", package.name, package.version),
                );
            }
            r.line(Stream::Out, "");
        }
        Event::NipmNotInstalled { count } => r.line(
            Stream::Out,
            &format!(
                "note: the manifest also lists {count} [nipm.dependencies] — those are NI Package\n      \
                 Manager packages and lvpm does not install them.\n"
            ),
        ),
        Event::PackageAlreadyInstalled { name, version } => r.line(
            Stream::Out,
            &format!("= {name} {version} already installed"),
        ),
        Event::PackageNotDowngraded {
            name,
            installed,
            wanted,
        } => r.line(
            Stream::Out,
            &format!(
                "= {name} {installed} installed, newer than {wanted} — not downgrading (pass --allow-downgrade)"
            ),
        ),
        Event::PackageReplacing {
            name,
            installed,
            wanted,
            upgrade,
            old_files,
            dry_run,
        } => {
            let mark = if *upgrade { "^" } else { "v" };
            let what = match dry_run {
                true => format!("{old_files} old files would be removed first"),
                false => "removing the old version first".to_string(),
            };
            r.line(
                Stream::Out,
                &format!("{mark} {name} {installed} -> {wanted}: {what}"),
            );
        }
        Event::PackageStart {
            name,
            version,
            dry_run,
        } => {
            let mark = if *dry_run { "?" } else { "+" };
            r.begin(Stream::Out, &format!("{mark} {name} {version} ... "));
        }
        Event::PackageDone {
            files,
            dry_run: true,
        } => r.end(Stream::Out, &format!("{files} files would be written")),
        Event::PackageDone {
            files,
            dry_run: false,
        } => r.end(Stream::Out, &format!("{files} files")),
        Event::PlannedWrites { files } => planned_writes(r, files),
        Event::FilesKept { count } => r.line(
            Stream::Out,
            &format!("      {count} existing file(s) left alone (Replace Mode = If Newer)"),
        ),
        Event::GroupSkipped { target_dir, files } => r.line(
            Stream::Out,
            &format!(
                "      {files} file(s) for {target_dir} skipped — a venv holds LabVIEW-tree content only"
            ),
        ),
        Event::MissingFromArchive { entry } => r.line(
            Stream::Out,
            &format!("      ! listed in spec but absent from archive: {entry}"),
        ),
        Event::HooksNotRun { hooks } => r.line(
            Stream::Out,
            &format!("      hooks not run: {}", hook_names(hooks)),
        ),
        Event::InstallDone {
            files,
            dry_run: true,
        } => r.line(
            Stream::Out,
            &format!("\ndry run: {files} files would be written, nothing changed"),
        ),
        Event::InstallDone {
            files,
            dry_run: false,
        } => r.line(Stream::Out, &format!("\ndone. {files} files written.")),
        Event::PhaseSkipped {
            phase,
            reason,
            skipped,
        } => phase_skipped(r, *phase, *reason, *skipped),
        Event::LaunchHint => r.line(
            Stream::Out,
            "\nnext: `lvpm launch` starts LabVIEW on this venv — a LabVIEW already open on it\n      \
             has to be restarted to see what was just installed",
        ),
        _ => unreachable!("routed here by Renderer::render: {event:?}"),
    }
}

fn planned_writes(r: &mut Renderer, files: &[PlannedFile]) {
    for file in files.iter().take(PREVIEW_LIMIT) {
        let overwrites = if file.overwrites {
            "   (overwrites)"
        } else {
            ""
        };
        r.line(
            Stream::Out,
            &format!("      {}{overwrites}", file.path.display()),
        );
    }
    if files.len() > PREVIEW_LIMIT {
        r.line(
            Stream::Out,
            &format!("      ... and {} more", files.len() - PREVIEW_LIMIT),
        );
    }
}

/// A phase that left nothing undone says nothing, and so does one whose
/// skipping was never worth a line: a scratch tree's refresh.
fn phase_skipped(r: &mut Renderer, phase: Phase, reason: SkipReason, skipped: usize) {
    if skipped == 0 {
        return;
    }
    let text = match (phase, reason) {
        (Phase::Relink, SkipReason::ScratchDestination) => {
            "\nrelink: skipped — a scratch tree has no LabVIEW to relink with"
        }
        (Phase::Relink, SkipReason::Headless) => {
            "\nrelink: skipped — LabVIEW runs headless here (LV_RTE_HEADLESS); a compile or build\n      \
             resolves the links as it loads. --relink forces the pass, `lvpm relink --all` runs it later."
        }
        (Phase::Relink, SkipReason::Declined) => {
            "\nrelink: skipped (--no-relink). These VIs still declare the paths their\n      \
             build machine wrote, so run `lvpm relink <package>` before using them."
        }
        (Phase::Hooks, SkipReason::ScratchDestination) => {
            "\npost-install hooks: skipped — a scratch tree has no LabVIEW to run them"
        }
        (Phase::Hooks, SkipReason::Headless) => {
            "\nhooks: skipped — LabVIEW runs headless here (LV_RTE_HEADLESS). Install hooks mostly add\n      \
             palettes, menus or markers for the IDE; a package whose hook does real setup needs --hooks."
        }
        (Phase::Refresh, SkipReason::Headless) => {
            "\npalettes and menus: not refreshed — LabVIEW runs headless here (`lvpm refresh` if wanted)"
        }
        _ => return,
    };
    r.line(Stream::Out, text);
}

#[cfg(test)]
mod tests {
    use super::super::tests::renderer;
    use super::*;
    use lvpm_core::events::PackageRef;
    use std::path::PathBuf;

    fn text_of(events: &[Event]) -> (String, String) {
        let (mut renderer, out, err) = renderer();
        for event in events {
            renderer.render(event);
        }
        (out.text(), err.text())
    }

    #[test]
    fn what_the_install_reads_and_where_it_goes_is_on_stderr() {
        let (out, err) = text_of(&[
            Event::ManifestInUse {
                path: PathBuf::from("proj").join("lvpm.toml"),
                name: Some("demo".into()),
                dependencies: 1,
                labview: Some("2024".into()),
            },
            Event::InstallTarget {
                labview: Some("LabVIEW 2026 (64-bit)  v26.3".into()),
                path: PathBuf::from("LV"),
            },
            Event::InstallTarget {
                labview: None,
                path: PathBuf::from("dest").join("LabVIEW"),
            },
            Event::IndexesLoading,
            Event::IndexesLoaded { versions: 4 },
        ]);
        assert_eq!(out, "");
        assert_eq!(
            err,
            format!(
                "manifest: {} (demo) — 1 dependency\n\
                 \x20     manifest says labview = \"2024\" (minimum)\n\
                 target: LabVIEW 2026 (64-bit)  v26.3  (LV)\n\
                 target: scratch tree at {}\n\
                 loading indexes...\n  4 package versions known\n",
                PathBuf::from("proj").join("lvpm.toml").display(),
                PathBuf::from("dest").join("LabVIEW").display()
            )
        );
    }

    #[test]
    fn a_resolution_is_a_counted_list_and_a_blank_line() {
        let package = |name: &str, version: &str| PackageRef {
            name: name.into(),
            version: version.into(),
        };
        let (out, _) = text_of(&[Event::Resolved {
            packages: vec![package("alpha", "2.0"), package("beta", "1.0")],
        }]);
        assert_eq!(out, "\nresolved 2 packages:\n  alpha 2.0\n  beta 1.0\n\n");
        let (out, _) = text_of(&[Event::Resolved {
            packages: vec![package("alpha", "2.0")],
        }]);
        assert!(out.starts_with("\nresolved 1 package:\n"), "{out}");
    }

    #[test]
    fn a_package_is_one_line_in_two_parts_that_a_pre_install_hook_extends() {
        let (out, err) = text_of(&[
            Event::PackageStart {
                name: "alpha".into(),
                version: "2.0".into(),
                dry_run: false,
            },
            Event::HookRan {
                package: "alpha".into(),
                hook: lvpm_core::events::HookKind::PreInstall,
                took: std::time::Duration::from_millis(1_250),
            },
            Event::PackageDone {
                files: 3,
                dry_run: false,
            },
        ]);
        assert_eq!(out, "+ alpha 2.0 ... [pre-install ok, 1.2s] 3 files\n");
        assert_eq!(err, "");
    }

    #[test]
    fn a_dry_run_lists_six_planned_writes_and_counts_the_rest() {
        let files: Vec<PlannedFile> = (1..=8)
            .map(|n| PlannedFile {
                path: PathBuf::from(format!("f{n}")),
                overwrites: n == 2,
            })
            .collect();
        let (out, _) = text_of(&[
            Event::PackageStart {
                name: "alpha".into(),
                version: "2.0".into(),
                dry_run: true,
            },
            Event::PackageDone {
                files: 8,
                dry_run: true,
            },
            Event::PlannedWrites { files },
        ]);
        assert_eq!(
            out,
            "? alpha 2.0 ... 8 files would be written\n      f1\n      f2   (overwrites)\n      f3\n      f4\n      f5\n      f6\n      ... and 2 more\n"
        );
    }

    #[test]
    fn a_replaced_package_says_which_way_and_whether_it_happened() {
        let replacing = |upgrade, dry_run| Event::PackageReplacing {
            name: "alpha".into(),
            installed: "1.0".into(),
            wanted: "2.0".into(),
            upgrade,
            old_files: 4,
            dry_run,
        };
        let (out, _) = text_of(&[replacing(true, false), replacing(false, true)]);
        assert_eq!(
            out,
            "^ alpha 1.0 -> 2.0: removing the old version first\n\
             v alpha 1.0 -> 2.0: 4 old files would be removed first\n"
        );
    }

    #[test]
    fn the_totals_differ_on_a_dry_run() {
        let (out, _) = text_of(&[
            Event::InstallDone {
                files: 2,
                dry_run: true,
            },
            Event::InstallDone {
                files: 2,
                dry_run: false,
            },
        ]);
        assert_eq!(
            out,
            "\ndry run: 2 files would be written, nothing changed\n\ndone. 2 files written.\n"
        );
    }

    #[test]
    fn a_skipped_phase_speaks_only_when_something_was_left_undone() {
        let skipped = |phase, reason, skipped| Event::PhaseSkipped {
            phase,
            reason,
            skipped,
        };
        let (out, err) = text_of(&[
            skipped(Phase::Relink, SkipReason::ScratchDestination, 0),
            skipped(Phase::Hooks, SkipReason::ScratchDestination, 0),
            skipped(Phase::Refresh, SkipReason::ScratchDestination, 3),
            skipped(Phase::Relink, SkipReason::ScratchDestination, 2),
            skipped(Phase::Hooks, SkipReason::ScratchDestination, 1),
        ]);
        assert_eq!(
            out,
            "\nrelink: skipped — a scratch tree has no LabVIEW to relink with\n\
             \npost-install hooks: skipped — a scratch tree has no LabVIEW to run them\n"
        );
        assert_eq!(err, "");
    }

    #[test]
    fn hooks_not_run_are_named_without_the_path_they_had_on_the_build_machine() {
        let (out, _) = text_of(&[Event::HooksNotRun {
            hooks: vec![
                "PostInstall=<OS Boot Volume Root>\\ci\\PostInstall.vi".into(),
                "PreUninstall=x.vi".into(),
            ],
        }]);
        assert_eq!(out, "      hooks not run: PostInstall, PreUninstall\n");
    }
}
