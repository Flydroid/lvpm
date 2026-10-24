//! The wording of `lvpm sync`: its events, and the errors it ends in. Core's
//! errors say what went wrong; the flags and the commands to type are the CLI's.

use super::lock::{error as lock_error, stale_reason};
use super::{Renderer, Stream};
use lvpm_core::errors::{
    ArchiveFault, LockError, LockIncompatible, LockedArchiveProblem, RouteError, SyncError,
};
use lvpm_core::events::{Event, PackageRef, RemoveReason, SyncStep};

pub(super) fn render(r: &mut Renderer, event: &Event) {
    match event {
        Event::SyncLockMissing { dry_run: false, .. } => r.line(
            Stream::Out,
            "no lvpm.lock found: writing one from lvpm.toml (commit lvpm.lock with the project)",
        ),
        Event::SyncLockMissing { dry_run: true, .. } => r.line(
            Stream::Out,
            "no lvpm.lock found: one would be written from lvpm.toml",
        ),
        Event::SyncUnresolved { name, required_by } => r.line(
            Stream::Err,
            &format!(
                "warning: {name}, required by {required_by}, is not installed: no source \
                 listed it when lvpm.lock was made"
            ),
        ),
        Event::SyncMigration { removing } => r.line(Stream::Out, &migration(removing)),
        Event::SyncPlanned { steps, dry_run } => {
            r.line(Stream::Out, &plan_heading(steps));
            for step in steps {
                r.line(Stream::Out, &format!("  {}", step_line(step)));
            }
            if *dry_run {
                r.line(Stream::Out, "dry run: nothing changed");
            }
        }
        Event::SyncNothingToDo => r.line(
            Stream::Out,
            "nothing to do: the destination already matches lvpm.lock",
        ),
        _ => unreachable!("routed here by Renderer::render: {event:?}"),
    }
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

/// The first sync of a project: what the new lock does not list goes.
fn migration(removing: &[PackageRef]) -> String {
    let names: Vec<String> = removing
        .iter()
        .map(|package| format!("{} {}", package.name, package.version))
        .collect();
    format!(
        "note: {} installed package{} not in lvpm.lock will be removed: {}",
        removing.len(),
        plural(removing.len()),
        names.join(", ")
    )
}

/// `sync: 2 to install, 1 to remove`: what is going to happen, in counts.
fn plan_heading(steps: &[SyncStep]) -> String {
    let count = |wanted: fn(&SyncStep) -> bool| steps.iter().filter(|step| wanted(step)).count();
    let parts: Vec<String> = [
        (count(|s| matches!(s, SyncStep::Install { .. })), "install"),
        (count(|s| matches!(s, SyncStep::Replace { .. })), "replace"),
        (count(|s| matches!(s, SyncStep::Remove { .. })), "remove"),
    ]
    .into_iter()
    .filter(|(number, _)| *number > 0)
    .map(|(number, what)| format!("{number} to {what}"))
    .collect();
    format!("sync: {}", parts.join(", "))
}

/// One change, with its reason where it has one.
fn step_line(step: &SyncStep) -> String {
    match step {
        SyncStep::Install { name, version } => format!("install {name} {version}"),
        SyncStep::Replace { name, from, to } => format!("replace {name} {from} -> {to}"),
        SyncStep::Remove {
            name,
            version,
            reason,
        } => {
            let why = match reason {
                RemoveReason::NotInLock => "not in lvpm.lock",
                RemoveReason::DevOnly => "only dev dependencies need it, and --no-dev is given",
            };
            format!("remove {name} {version} ({why})")
        }
    }
}

/// What `lvpm sync` prints when it fails: what happened, then the next thing
/// to do.
pub fn error(error: &SyncError) -> String {
    match error {
        SyncError::Lock(LockError::NoLock { .. }) => {
            "there is no lvpm.lock\nnext: run `lvpm lock`, and commit lvpm.lock".to_string()
        }
        SyncError::Lock(LockError::Stale(reason)) => format!(
            "lvpm.lock is out of date: {}\nnext: run `lvpm lock` and commit lvpm.lock",
            stale_reason(reason)
        ),
        SyncError::Lock(other) => lock_error(other),
        SyncError::Archives(problems) => archives_text(problems),
        SyncError::Incomplete { done, left, cause } => incomplete_text(done, left, cause),
        SyncError::Other(cause) => format!("{cause:#}"),
    }
}

/// What `lvpm sync` prints when it fails on a headless machine where only
/// headless made it `--locked`: for a lock that is missing or out of date, the
/// way out is to lock where a person is, or to pass `--no-locked`.
pub fn headless_error(error: &SyncError) -> String {
    match error {
        SyncError::Lock(LockError::NoLock { .. }) => {
            "no lvpm.lock: run `lvpm lock` locally and commit it, or pass --no-locked".to_string()
        }
        SyncError::Lock(LockError::Stale(reason)) => format!(
            "lvpm.lock is out of date: {}\n\
             next: run `lvpm lock` locally and commit lvpm.lock, or pass --no-locked",
            stale_reason(reason)
        ),
        other => self::error(other),
    }
}

fn archives_text(problems: &[LockedArchiveProblem]) -> String {
    let mut text = format!(
        "cannot sync: {} locked archive{} could not be had, and nothing was changed:",
        problems.len(),
        plural(problems.len())
    );
    for problem in problems {
        let LockedArchiveProblem {
            name,
            version,
            sha256,
            fault,
        } = problem;
        text.push_str("\n  ");
        text.push_str(&match fault {
            ArchiveFault::NotFound => format!(
                "{name} {version}: no archive with SHA-256 {sha256} is in the cache or in any source"
            ),
            ArchiveFault::DiffersFromLock { found_sha256 } => format!(
                "the archive of {name} {version} does not match lvpm.lock: it has SHA-256 \
                 {found_sha256} and the lock has {sha256}, so it was not installed"
            ),
        });
    }
    text.push_str(
        "\nnext: add the archive lvpm.lock was made from with `lvpm cache add <file>`, or list a \
         folder that holds it under sources.local (`lvpm config set sources.local <dir>`), then \
         run `lvpm sync` again",
    );
    text
}

fn incomplete_text(done: &[SyncStep], left: &[SyncStep], cause: &str) -> String {
    let mut text = incomplete_body(done, left, cause);
    text.push_str("\nnext: fix what is wrong, then re-run lvpm sync to finish");
    text
}

/// What a sync that stopped part-way did and did not do, without what to do
/// about it.
pub(super) fn incomplete_body(done: &[SyncStep], left: &[SyncStep], cause: &str) -> String {
    let mut text = format!("sync stopped before it was done: {cause}");
    for (heading, steps) in [("done:", done), ("not done:", left)] {
        if steps.is_empty() {
            continue;
        }
        text.push('\n');
        text.push_str(heading);
        for step in steps {
            text.push_str(&format!("\n  {}", step_line(step)));
        }
    }
    text
}

/// What `lvpm install` prints when it cannot tell which operation it means.
pub fn route_error(error: &RouteError) -> String {
    match error {
        RouteError::NoPackage { searched_from } => format!(
            "give a package name, or --manifest <FILE>\n\
             there is no project to sync: no lvpm.toml found in {} or above\n\
             next: name a package to install, or run this in a project, a directory with an \
             lvpm.toml",
            searched_from.display()
        ),
        RouteError::NotWithLock { option, adding } => {
            let (flag, next) = match (option, adding) {
                (LockIncompatible::NoDeps, false) => (
                    "--no-deps",
                    "drop --no-deps, or install one package where you choose: \
                     `lvpm install <package> --no-deps --prefix <dir>`",
                ),
                (LockIncompatible::NoDeps, true) => (
                    "--no-deps",
                    "drop --no-deps, or say where the package goes: \
                     `lvpm install <package> --no-deps --prefix <dir>`",
                ),
                (LockIncompatible::Manifest, _) => (
                    "--manifest",
                    "drop --manifest to sync this project, or name a package and where it \
                     goes: `lvpm install <package> --prefix <dir>`",
                ),
            };
            format!("{flag} is {error}\nnext: {next}")
        }
        RouteError::Other(cause) => format!("{cause:#}"),
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::renderer;
    use super::*;
    use lvpm_core::lockfile::StaleReason;
    use std::path::PathBuf;

    fn install(name: &str, version: &str) -> SyncStep {
        SyncStep::Install {
            name: name.into(),
            version: version.into(),
        }
    }

    fn remove(name: &str, version: &str, reason: RemoveReason) -> SyncStep {
        SyncStep::Remove {
            name: name.into(),
            version: version.into(),
            reason,
        }
    }

    #[test]
    fn a_plan_lists_each_step_with_the_reason_a_removal_has_and_a_dry_run_says_it_changed_nothing()
    {
        let (mut renderer, out, err) = renderer();
        renderer.render(&Event::SyncPlanned {
            steps: vec![
                remove("delta", "1.0", RemoveReason::NotInLock),
                SyncStep::Replace {
                    name: "alpha".into(),
                    from: "1.0".into(),
                    to: "2.0".into(),
                },
                install("beta", "1.0"),
            ],
            dry_run: true,
        });
        assert_eq!(
            out.text(),
            "sync: 1 to install, 1 to replace, 1 to remove\n\
             \x20 remove delta 1.0 (not in lvpm.lock)\n\
             \x20 replace alpha 1.0 -> 2.0\n\
             \x20 install beta 1.0\n\
             dry run: nothing changed\n"
        );
        assert_eq!(err.text(), "");
    }

    #[test]
    fn a_dev_only_removal_says_which_flag_asked_for_it() {
        assert_eq!(
            step_line(&remove("caraya", "1.0", RemoveReason::DevOnly)),
            "remove caraya 1.0 (only dev dependencies need it, and --no-dev is given)"
        );
    }

    #[test]
    fn the_first_sync_says_what_it_takes_out_and_to_commit_the_lock() {
        let (mut renderer, out, _) = renderer();
        renderer.render(&Event::SyncLockMissing {
            path: PathBuf::from("lvpm.lock"),
            dry_run: false,
        });
        renderer.render(&Event::SyncMigration {
            removing: vec![PackageRef {
                name: "gamma".into(),
                version: "1.0".into(),
            }],
        });
        assert_eq!(
            out.text(),
            "no lvpm.lock found: writing one from lvpm.toml (commit lvpm.lock with the project)\n\
             note: 1 installed package not in lvpm.lock will be removed: gamma 1.0\n"
        );
    }

    #[test]
    fn a_requirement_nobody_listed_is_a_warning_on_stderr_and_nothing_to_do_is_a_line_on_stdout() {
        let (mut renderer, out, err) = renderer();
        renderer.render(&Event::SyncUnresolved {
            name: "ni_lib_missing".into(),
            required_by: "beta".into(),
        });
        renderer.render(&Event::SyncNothingToDo);
        assert_eq!(
            err.text(),
            "warning: ni_lib_missing, required by beta, is not installed: no source listed it \
             when lvpm.lock was made\n"
        );
        assert_eq!(
            out.text(),
            "nothing to do: the destination already matches lvpm.lock\n"
        );
    }

    #[test]
    fn a_lock_a_headless_machine_refuses_says_to_lock_locally_or_pass_no_locked() {
        let missing = headless_error(&SyncError::Lock(LockError::NoLock {
            path: PathBuf::from("lvpm.lock"),
        }));
        assert_eq!(
            missing,
            "no lvpm.lock: run `lvpm lock` locally and commit it, or pass --no-locked"
        );
        let stale = headless_error(&SyncError::Lock(LockError::Stale(
            StaleReason::LabviewChanged,
        )));
        assert_eq!(
            stale,
            "lvpm.lock is out of date: labview changed\n\
             next: run `lvpm lock` locally and commit lvpm.lock, or pass --no-locked"
        );
    }

    #[test]
    fn a_failure_headless_did_not_cause_reads_as_it_does_anywhere() {
        let failure = SyncError::Incomplete {
            done: vec![],
            left: vec![install("beta", "1.0")],
            cause: "it broke".into(),
        };
        assert_eq!(headless_error(&failure), error(&failure));
    }

    #[test]
    fn a_missing_archive_names_the_package_version_and_hash_and_how_to_supply_it() {
        let text = error(&SyncError::Archives(vec![LockedArchiveProblem {
            name: "alpha".into(),
            version: "2.0".into(),
            sha256: "ab".repeat(32),
            fault: ArchiveFault::NotFound,
        }]));
        for part in [
            "alpha 2.0",
            &"ab".repeat(32),
            "nothing was changed",
            "`lvpm cache add <file>`",
            "sources.local",
        ] {
            assert!(text.contains(part), "{part:?} is not in:\n{text}");
        }
    }

    #[test]
    fn an_archive_with_other_bytes_is_said_not_to_match_the_lock() {
        let text = error(&SyncError::Archives(vec![LockedArchiveProblem {
            name: "alpha".into(),
            version: "2.0".into(),
            sha256: "ab".repeat(32),
            fault: ArchiveFault::DiffersFromLock {
                found_sha256: "cd".repeat(32),
            },
        }]));
        assert!(
            text.contains("the archive of alpha 2.0 does not match lvpm.lock"),
            "{text}"
        );
        assert!(text.contains(&"cd".repeat(32)), "{text}");
    }

    #[test]
    fn a_sync_that_stopped_lists_what_is_done_and_what_is_not_and_ends_with_how_to_finish() {
        let text = error(&SyncError::Incomplete {
            done: vec![install("alpha", "2.0")],
            left: vec![install("beta", "1.0")],
            cause: "it broke".into(),
        });
        assert_eq!(
            text,
            "sync stopped before it was done: it broke\n\
             done:\n  install alpha 2.0\n\
             not done:\n  install beta 1.0\n\
             next: fix what is wrong, then re-run lvpm sync to finish"
        );
    }

    #[test]
    fn a_missing_or_stale_lock_says_to_lock_and_commit() {
        let missing = error(&SyncError::Lock(LockError::NoLock {
            path: PathBuf::from("lvpm.lock"),
        }));
        assert!(missing.starts_with("there is no lvpm.lock"), "{missing}");
        let stale = error(&SyncError::Lock(LockError::Stale(
            lvpm_core::lockfile::StaleReason::DependencyAdded {
                name: "gamma".into(),
                dev: false,
            },
        )));
        assert_eq!(
            stale,
            "lvpm.lock is out of date: dependency \"gamma\" was added\n\
             next: run `lvpm lock` and commit lvpm.lock"
        );
    }

    #[test]
    fn a_flag_a_lock_cannot_honor_is_named_with_what_to_do_instead() {
        for adding in [false, true] {
            let text = route_error(&RouteError::NotWithLock {
                option: LockIncompatible::NoDeps,
                adding,
            });
            assert!(
                text.starts_with(
                    "--no-deps is not supported with a lock (lvpm.lock always holds the full set)\nnext: "
                ),
                "{text}"
            );
            // The way out of it is never the install it refuses.
            assert!(
                text.ends_with("`lvpm install <package> --no-deps --prefix <dir>`"),
                "{text}"
            );
        }
    }

    #[test]
    fn nothing_to_install_outside_a_project_starts_with_the_text_it_always_had_and_says_where_it_looked()
     {
        let text = route_error(&RouteError::NoPackage {
            searched_from: PathBuf::from("here"),
        });
        assert!(
            text.starts_with("give a package name, or --manifest <FILE>\n"),
            "{text}"
        );
        assert!(
            text.contains("no lvpm.toml found in here or above"),
            "{text}"
        );
        assert!(text.lines().last().unwrap().starts_with("next: "), "{text}");
    }
}
