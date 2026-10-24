//! The wording of `lvpm lock`: its events, and the errors it ends in. Core's
//! errors say what went wrong; the hint that ends each message, the name of
//! the manifest and the commands to type are the CLI's.

use super::{Renderer, Stream};
use lvpm_core::errors::{ArchiveFailure, LockError};
use lvpm_core::events::{Event, LockChange};
use lvpm_core::lockfile::StaleReason;
use lvpm_core::solve::{Conflict, Fact};

pub(super) fn render(r: &mut Renderer, event: &Event) {
    match event {
        Event::LockHashing {
            name,
            version,
            position,
            total,
        } => r.line(
            Stream::Err,
            &format!("hashing {name} {version} ({position}/{total})"),
        ),
        Event::RequirementUnresolved { name, required_by } => r.line(
            Stream::Err,
            &format!(
                "warning: {name}, required by {required_by}, is in no source: it is recorded \
                 as unresolved and not locked"
            ),
        ),
        Event::LockChanged { changes } => {
            for change in changes {
                r.line(Stream::Out, &change_line(change));
            }
        }
        Event::LockWritten { path, packages } => r.line(
            Stream::Out,
            &format!(
                "wrote {} ({packages} package{}); commit it with lvpm.toml",
                path.display(),
                if *packages == 1 { "" } else { "s" }
            ),
        ),
        Event::LockUpToDate { path } => {
            r.line(Stream::Out, &format!("{} is up to date", path.display()))
        }
        _ => unreachable!("routed here by Renderer::render: {event:?}"),
    }
}

/// Why a lock is out of date, as the clause that follows "lvpm.lock is out of
/// date:". It names the manifest line at fault.
pub fn stale_reason(reason: &StaleReason) -> String {
    let dependency = |dev: bool| if dev { "dev dependency" } else { "dependency" };
    match reason {
        StaleReason::FormatOutdated => "the lock format is outdated".to_string(),
        StaleReason::LabviewChanged => "labview changed".to_string(),
        StaleReason::SourcesChanged => "sources changed".to_string(),
        StaleReason::DependencyAdded { name, dev } => {
            format!("{} \"{name}\" was added", dependency(*dev))
        }
        StaleReason::DependencyRemoved { name, dev } => {
            format!("{} \"{name}\" was removed", dependency(*dev))
        }
        StaleReason::ConstraintExcludesLocked {
            name,
            constraint,
            locked,
            dev,
        } => {
            let prefix = if *dev { "dev dependency " } else { "" };
            format!("{prefix}\"{name}\" \"{constraint}\" excludes locked {locked}")
        }
    }
}

fn change_line(change: &LockChange) -> String {
    match change {
        LockChange::Added { name, version } => format!("+ {name} {version}"),
        LockChange::Removed { name, version } => format!("- {name} {version}"),
        LockChange::Changed { name, from, to } => format!("~ {name} {from} -> {to}"),
    }
}

/// What `lvpm lock` prints when it fails: what happened, then the next thing
/// to do.
pub fn error(error: &LockError) -> String {
    match error {
        LockError::NoManifest { .. } => {
            format!("{error}\nnext: run `lvpm lock` in a project, a directory with an lvpm.toml")
        }
        LockError::NoLabview => format!(
            "{error}, so lvpm does not know which LabVIEW to lock for\n\
             next: add labview = \"<year>\" under [project] in lvpm.toml (for example \
             labview = \"2024\"), then run `lvpm lock` again"
        ),
        LockError::LabviewNotAYear { .. } => format!(
            "{error}\n\
             next: write labview = \"<year>\" under [project] in lvpm.toml (for example \
             labview = \"2024\"), then run `lvpm lock` again"
        ),
        LockError::Conflict(conflict) => conflict_text(conflict),
        LockError::ArchivesUnavailable(failures) => archives_text(failures),
        // `lvpm lock --check` fails with one line: a script reads it.
        LockError::NoLock { .. } => {
            "there is no lvpm.lock; run `lvpm lock` to create it".to_string()
        }
        LockError::UnreadableLock { reason, .. } => format!(
            "lvpm.lock cannot be read ({}); run `lvpm lock` to regenerate it",
            reason.lines().next().unwrap_or_default()
        ),
        LockError::Stale(reason) => format!(
            "lvpm.lock is out of date: {}; run `lvpm lock` to update it",
            stale_reason(reason)
        ),
        LockError::Cancelled => {
            format!("{error}; lvpm.lock is unchanged\nnext: run `lvpm lock` again")
        }
        LockError::Other(cause) => format!("{cause:#}"),
    }
}

fn conflict_text(conflict: &Conflict) -> String {
    let unlisted: Vec<&str> = conflict
        .facts
        .iter()
        .filter_map(|fact| match fact {
            Fact::NotListed { package } => Some(package.as_str()),
            _ => None,
        })
        .collect();
    let only_unlisted = unlisted.len() == conflict.facts.len();
    let mut text = match only_unlisted {
        true => "cannot lock:".to_string(),
        false => "cannot lock: no set of versions satisfies lvpm.toml:".to_string(),
    };
    for fact in &conflict.facts {
        text.push_str("\n  ");
        text.push_str(&fact_line(fact));
    }
    text.push_str("\nnext: ");
    match unlisted.is_empty() {
        true => text.push_str("change one of these in lvpm.toml, then run `lvpm lock` again"),
        false => {
            let searches: Vec<String> = unlisted
                .iter()
                .map(|package| format!("`lvpm search {package}`"))
                .collect();
            text.push_str(&format!(
                "look for it with {}, or list a repository under [sources] in lvpm.toml, \
                 then run `lvpm lock` again",
                searches.join(", ")
            ));
        }
    }
    text
}

/// A fact as one line of a report. What the manifest asks for is shown the way
/// the manifest line is written.
fn fact_line(fact: &Fact) -> String {
    match fact {
        Fact::Manifest { package, wanted } => {
            format!("lvpm.toml: {package} = \"{}\"", manifest_constraint(wanted))
        }
        other => other.to_string(),
    }
}

/// A version set as a manifest constraint reads: `= 1.0` is `1.0`, `>= 2.0`
/// is `>=2.0`, `*` stays. Anything else is shown as the solver describes it.
fn manifest_constraint(wanted: &str) -> String {
    if let Some(exact) = wanted.strip_prefix("= ") {
        return exact.to_string();
    }
    match wanted.strip_prefix(">= ") {
        Some(floor) if !floor.contains(',') => format!(">={floor}"),
        _ => wanted.to_string(),
    }
}

fn archives_text(failures: &[ArchiveFailure]) -> String {
    let plural = if failures.len() == 1 { "" } else { "s" };
    let mut text = format!(
        "cannot lock: {} archive{plural} could not be fetched:",
        failures.len()
    );
    for failure in failures {
        text.push_str(&format!(
            "\n  {} {}: {}",
            failure.name, failure.version, failure.reason
        ));
    }
    text.push_str(
        "\nnext: make each source reachable, or add the archive with `lvpm cache add <file>`, \
         then run `lvpm lock` again",
    );
    text
}

#[cfg(test)]
mod tests {
    use super::super::tests::renderer;
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn progress_and_warnings_go_to_stderr_and_the_result_to_stdout() {
        let (mut renderer, out, err) = renderer();
        renderer.render(&Event::LockHashing {
            name: "alpha".into(),
            version: "2.0".into(),
            position: 3,
            total: 48,
        });
        renderer.render(&Event::RequirementUnresolved {
            name: "ni_lib_missing".into(),
            required_by: "beta".into(),
        });
        renderer.render(&Event::LockChanged {
            changes: vec![
                LockChange::Added {
                    name: "alpha".into(),
                    version: "2.0".into(),
                },
                LockChange::Changed {
                    name: "beta".into(),
                    from: "1.0".into(),
                    to: "1.1".into(),
                },
                LockChange::Removed {
                    name: "gamma".into(),
                    version: "1.5".into(),
                },
            ],
        });
        renderer.render(&Event::LockWritten {
            path: PathBuf::from("lvpm.lock"),
            packages: 2,
        });
        assert_eq!(
            err.text(),
            "hashing alpha 2.0 (3/48)\n\
             warning: ni_lib_missing, required by beta, is in no source: it is recorded as \
             unresolved and not locked\n"
        );
        assert_eq!(
            out.text(),
            "+ alpha 2.0\n~ beta 1.0 -> 1.1\n- gamma 1.5\n\
             wrote lvpm.lock (2 packages); commit it with lvpm.toml\n"
        );
    }

    #[test]
    fn a_lock_left_alone_says_it_is_up_to_date() {
        let (mut renderer, out, err) = renderer();
        renderer.render(&Event::LockUpToDate {
            path: PathBuf::from("lvpm.lock"),
        });
        assert_eq!(out.text(), "lvpm.lock is up to date\n");
        assert_eq!(err.text(), "");
    }

    #[test]
    fn every_stale_reason_names_the_line_at_fault() {
        let named = |name: &str| name.to_string();
        let excludes = |dev| StaleReason::ConstraintExcludesLocked {
            name: named("beta"),
            constraint: named("2.0"),
            locked: named("1.0"),
            dev,
        };
        for (reason, text) in [
            (
                StaleReason::DependencyAdded {
                    name: named("gamma"),
                    dev: false,
                },
                "dependency \"gamma\" was added",
            ),
            (
                StaleReason::DependencyRemoved {
                    name: named("beta"),
                    dev: false,
                },
                "dependency \"beta\" was removed",
            ),
            (
                StaleReason::DependencyAdded {
                    name: named("caraya"),
                    dev: true,
                },
                "dev dependency \"caraya\" was added",
            ),
            (
                StaleReason::DependencyRemoved {
                    name: named("caraya"),
                    dev: true,
                },
                "dev dependency \"caraya\" was removed",
            ),
            (excludes(false), "\"beta\" \"2.0\" excludes locked 1.0"),
            (
                excludes(true),
                "dev dependency \"beta\" \"2.0\" excludes locked 1.0",
            ),
            (StaleReason::LabviewChanged, "labview changed"),
            (StaleReason::SourcesChanged, "sources changed"),
            (StaleReason::FormatOutdated, "the lock format is outdated"),
        ] {
            assert_eq!(stale_reason(&reason), text, "{reason:?}");
        }
    }

    #[test]
    fn a_failed_check_is_one_line_that_ends_with_the_command_to_run() {
        let path = PathBuf::from("work/lvpm.lock");
        for (error, expected) in [
            (
                LockError::Stale(StaleReason::DependencyAdded {
                    name: "gamma".into(),
                    dev: false,
                }),
                "lvpm.lock is out of date: dependency \"gamma\" was added; \
                 run `lvpm lock` to update it",
            ),
            (
                LockError::NoLock { path: path.clone() },
                "there is no lvpm.lock; run `lvpm lock` to create it",
            ),
            (
                LockError::UnreadableLock {
                    path,
                    reason: "parsing lvpm.lock: TOML parse error at line 1\n  |\n1 | not toml"
                        .into(),
                },
                "lvpm.lock cannot be read (parsing lvpm.lock: TOML parse error at line 1); \
                 run `lvpm lock` to regenerate it",
            ),
        ] {
            assert_eq!(super::error(&error), expected);
        }
    }

    fn manifest(package: &str, wanted: &str) -> Fact {
        Fact::Manifest {
            package: package.into(),
            wanted: wanted.into(),
        }
    }

    #[test]
    fn a_conflict_names_the_manifest_lines_as_they_are_written() {
        let text = error(&LockError::Conflict(Conflict {
            facts: vec![
                manifest("A", "*"),
                manifest("C", "= 1.0"),
                manifest("D", ">= 2.0"),
                Fact::Requires {
                    package: "A".into(),
                    versions: "= 2.0".into(),
                    needs: "C".into(),
                    wanted: ">= 2.0".into(),
                },
            ],
        }));
        assert_eq!(
            text,
            "cannot lock: no set of versions satisfies lvpm.toml:\n\
             \x20 lvpm.toml: A = \"*\"\n\
             \x20 lvpm.toml: C = \"1.0\"\n\
             \x20 lvpm.toml: D = \">=2.0\"\n\
             \x20 A = 2.0 requires C >= 2.0\n\
             next: change one of these in lvpm.toml, then run `lvpm lock` again"
        );
    }

    #[test]
    fn a_package_in_no_source_comes_with_the_search_to_run() {
        let text = error(&LockError::Conflict(Conflict {
            facts: vec![
                Fact::NotListed {
                    package: "zeta".into(),
                },
                Fact::NotListed {
                    package: "omega".into(),
                },
            ],
        }));
        assert_eq!(
            text,
            "cannot lock:\n\
             \x20 zeta is in no configured source\n\
             \x20 omega is in no configured source\n\
             next: look for it with `lvpm search zeta`, `lvpm search omega`, or list a repository \
             under [sources] in lvpm.toml, then run `lvpm lock` again"
        );
    }

    #[test]
    fn every_unfetchable_archive_is_in_one_message() {
        let text = error(&LockError::ArchivesUnavailable(vec![
            ArchiveFailure {
                name: "alpha".into(),
                version: "2.0".into(),
                reason: "downloading http://h/a.vip: refused".into(),
            },
            ArchiveFailure {
                name: "gamma".into(),
                version: "1.5".into(),
                reason: "reading g.vip".into(),
            },
        ]));
        assert!(
            text.starts_with("cannot lock: 2 archives could not be fetched:\n"),
            "{text}"
        );
        assert!(
            text.contains("\n  alpha 2.0: downloading http://h/a.vip: refused\n"),
            "{text}"
        );
        assert!(text.contains("\n  gamma 1.5: reading g.vip\n"), "{text}");
        assert!(text.ends_with("then run `lvpm lock` again"), "{text}");
    }

    #[test]
    fn a_manifest_without_labview_is_told_what_to_add() {
        let text = error(&LockError::NoLabview);
        assert!(
            text.contains("add labview = \"<year>\" under [project] in lvpm.toml"),
            "{text}"
        );
        assert!(text.ends_with("then run `lvpm lock` again"), "{text}");
    }

    #[test]
    fn every_error_ends_with_the_next_command_to_run() {
        for error in [
            LockError::NoManifest {
                searched_from: PathBuf::from("here"),
            },
            LockError::NoLabview,
            LockError::LabviewNotAYear {
                value: "soon".into(),
            },
            LockError::Cancelled,
        ] {
            let text = super::error(&error);
            let last = text.lines().last().unwrap();
            assert!(
                last.starts_with("next: ") && last.contains("`lvpm lock`"),
                "{text}"
            );
        }
    }
}
