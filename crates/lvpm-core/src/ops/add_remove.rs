//! `add` and `remove`: change one entry of `lvpm.toml`, lock the result, and
//! sync the destination to it.
//!
//! The new manifest is text in memory until everything that can fail without
//! changing anything has passed: the lock is solved, every archive it holds is
//! in the cache and hashed, and the sync is planned and its archives fetched.
//! Only then are `lvpm.toml` and `lvpm.lock` written and the destination put
//! right, and if that fails, both files are put back byte for byte.

use super::env::{Destination, OpEnv};
use super::install::{InstallOptions, announce_target, finish};
use super::lock::{self as locking, announce_written, write_manifest_and_lock};
use super::session::InstallSession;
use super::snapshot::restoring_on_failure;
use super::sync::{SyncOptions, announce_migration, apply_sync, fetch_locked, plan_sync};
use crate::errors::{EditError, LockError, SyncError};
use crate::events::{Event, ManifestEdit, Phase};
use crate::index::Index;
use crate::install;
use crate::lockfile::{self, Lockfile, constraint_text};
use crate::manifest_edit::{self, DependencyTable};
use crate::project::{self, Constraint, Project};
use crate::solve;
use anyhow::{Context, anyhow};
use std::path::PathBuf;

/// Put `spec` (`name`, or `name@constraint`) in `table` of `lvpm.toml`, lock,
/// and sync the destination. Without a constraint the entry is a floor at the
/// version the new lock holds for it (`>=1.0`); with one, the constraint is
/// written as given. An entry that is already there has its constraint
/// replaced.
///
/// Both files are written only once the lock is made and every archive of the
/// sync is in hand. With `options.dry_run` nothing is written, and the manifest
/// line, the lock changes and the sync plan are reported instead.
pub fn add(
    env: &OpEnv,
    spec: &str,
    table: DependencyTable,
    options: &InstallOptions,
) -> Result<(), EditError> {
    let request = Request::parse(spec)?;
    let prepared = Prepared::of(env)?;
    let dest = env.destination()?;
    announce_project(env, &prepared, &dest);

    let index = env.load_index(Some(&prepared.found))?;
    let constraint = match request.constraint {
        Some(written) => written,
        None => floor(env, &prepared, &index, table, &request.name)?,
    };
    let edit = prepared.set(table, &request.name, &constraint)?;
    commit(env, prepared, edit, &index, dest, options)
}

/// Take `name` out of `table` of `lvpm.toml`, lock, and sync the destination.
/// A package that another locked package requires stays locked and installed,
/// and is reported as such.
///
/// Both files are written only once the lock is made and every archive of the
/// sync is in hand. With `options.dry_run` nothing is written, and the manifest
/// line, the lock changes and the sync plan are reported instead.
pub fn remove(
    env: &OpEnv,
    name: &str,
    table: DependencyTable,
    options: &InstallOptions,
) -> Result<(), EditError> {
    let prepared = Prepared::of(env)?;
    let edit = prepared.take_out(table, name)?;
    let dest = env.destination()?;
    announce_project(env, &prepared, &dest);

    let index = env.load_index(Some(&prepared.found))?;
    commit(env, prepared, edit, &index, dest, options)
}

/// What `add` was asked for: a package, and the constraint if one was written.
#[derive(Debug, PartialEq)]
struct Request {
    name: String,
    constraint: Option<String>,
}

impl Request {
    fn parse(spec: &str) -> Result<Request, EditError> {
        let (name, constraint) = match spec.split_once('@') {
            Some((name, constraint)) => (name.trim(), Some(constraint.trim())),
            None => (spec.trim(), None),
        };
        if name.is_empty() {
            return Err(anyhow!("{spec:?} is not a package name or name@constraint").into());
        }
        if let Some(written) = constraint {
            if written.is_empty() {
                return Err(EditError::EmptyConstraint {
                    name: name.to_string(),
                });
            }
            Constraint::parse(written)?;
        }
        Ok(Request {
            name: name.to_string(),
            constraint: constraint.map(str::to_string),
        })
    }
}

/// The project an edit is made to, as it is on disk before anything changes.
struct Prepared {
    /// The manifest's path, and what it says.
    found: (PathBuf, Project),
    /// The manifest's text, which an edit changes in memory.
    text: String,
    lock_path: PathBuf,
    /// The lock that is there, if it can be read.
    previous: Option<Lockfile>,
    /// There is no lock at all, so this is the project's first.
    lock_missing: bool,
}

impl Prepared {
    fn of(env: &OpEnv) -> Result<Prepared, EditError> {
        let found = locking::find_manifest(env)?;
        let text = std::fs::read_to_string(&found.0)
            .with_context(|| format!("reading manifest {}", found.0.display()))?;
        let lock_path = found.0.with_file_name(lockfile::FILE_NAME);
        let (previous, lock_missing) = match locking::read_lock(&lock_path) {
            Ok(lock) => (Some(lock), false),
            Err(LockError::NoLock { .. }) => (None, true),
            Err(_) => (None, false),
        };
        Ok(Prepared {
            found,
            text,
            lock_path,
            previous,
            lock_missing,
        })
    }

    /// The entry of `table` called `name` in any case, as the manifest has it.
    fn entry(&self, table: DependencyTable, name: &str) -> Option<&(String, Constraint)> {
        let entries = match table {
            DependencyTable::Dependencies => &self.found.1.dependencies,
            DependencyTable::DevDependencies => &self.found.1.dev_dependencies,
        };
        entries
            .iter()
            .find(|(entry, _)| entry.eq_ignore_ascii_case(name))
    }

    /// The manifest with `name` in `table` asking for `constraint`.
    fn set(&self, table: DependencyTable, name: &str, constraint: &str) -> Result<Edit, EditError> {
        let (key, change) = match self.entry(table, name) {
            Some((entry, old)) => {
                let from = constraint_text(old);
                let change = (from != constraint).then(|| ManifestEdit::Updated {
                    name: entry.clone(),
                    from,
                    to: constraint.to_string(),
                });
                (entry.as_str(), change)
            }
            None => (
                name,
                Some(ManifestEdit::Added {
                    name: name.to_string(),
                    constraint: constraint.to_string(),
                }),
            ),
        };
        let text = manifest_edit::set(&self.text, table, key, constraint)?;
        Edit::of(text, change, table)
    }

    /// The manifest without `name` in `table`; the error says where the name
    /// is when it is not there.
    fn take_out(&self, table: DependencyTable, name: &str) -> Result<Edit, EditError> {
        let Some((entry, constraint)) = self.entry(table, name) else {
            let other = match table {
                DependencyTable::Dependencies => DependencyTable::DevDependencies,
                DependencyTable::DevDependencies => DependencyTable::Dependencies,
            };
            return Err(match self.entry(other, name) {
                Some(_) => EditError::InOtherTable {
                    name: name.to_string(),
                    found_in: other,
                },
                None => EditError::NotADependency {
                    name: name.to_string(),
                },
            });
        };
        let change = ManifestEdit::Removed {
            name: entry.clone(),
            constraint: constraint_text(constraint),
        };
        let text = manifest_edit::remove(&self.text, table, entry)?;
        Edit::of(text, Some(change), table)
    }
}

/// The manifest after an edit: its text, what that reads as, and what changed.
struct Edit {
    text: String,
    project: Project,
    /// What the edit did to the manifest; `None` when it left it as it was.
    change: Option<ManifestEdit>,
    /// The entry is in `[dev-dependencies]`.
    dev: bool,
}

impl Edit {
    fn of(
        text: String,
        change: Option<ManifestEdit>,
        table: DependencyTable,
    ) -> Result<Edit, EditError> {
        let project = project::parse(&text).context("parsing the new manifest")?;
        Ok(Edit {
            text,
            project,
            change,
            dev: table == DependencyTable::DevDependencies,
        })
    }
}

/// The constraint `add` writes for a package it was given none for: a floor at
/// the version the lock would hold for it. A package no source lists is `*`,
/// so that locking says so.
fn floor(
    env: &OpEnv,
    prepared: &Prepared,
    index: &Index,
    table: DependencyTable,
    name: &str,
) -> Result<String, EditError> {
    let project = &prepared.found.1;
    let gate = locking::lv_gate(project.labview.as_deref().ok_or(LockError::NoLabview)?)?;
    let Some(newest) = index.versions_of(name).into_iter().next() else {
        return Ok("*".to_string());
    };
    if index.best(name, None, Some(gate)).is_none()
        && let Some(gated) = solve::gated(newest, gate)
    {
        return Err(LockError::NeedsNewerLabview {
            name: newest.name.clone(),
            version: newest.version.to_string(),
            needs_year: gated.needs_year,
            project_year: gated.project_year,
        }
        .into());
    }
    let any = prepared.set(table, name, "*")?;
    let resolved =
        locking::resolved_version(env, &any.project, index, prepared.previous.as_ref(), name)?;
    Ok(resolved.map_or_else(|| "*".to_string(), |version| format!(">={version}")))
}

/// What the edit reads and where it goes, as `sync` says it first.
fn announce_project(env: &OpEnv, prepared: &Prepared, dest: &Destination) {
    let (path, project) = &prepared.found;
    env.sink.emit(Event::PhaseStarted {
        phase: Phase::Resolve,
    });
    env.sink.emit(Event::ManifestInUse {
        path: path.clone(),
        name: project.name.clone(),
        dependencies: project.dependencies.len(),
        labview: project.labview.clone(),
    });
    announce_target(env, &dest.store());
}

/// Lock the edited manifest, plan and fetch the sync, and only then write the
/// files and put the destination right.
fn commit(
    env: &OpEnv,
    prepared: Prepared,
    edit: Edit,
    index: &Index,
    dest: Destination,
    options: &InstallOptions,
) -> Result<(), EditError> {
    let Prepared {
        found: (manifest_path, _),
        lock_path,
        previous,
        lock_missing,
        text: before,
    } = prepared;
    if lock_missing {
        env.sink.emit(Event::SyncLockMissing {
            path: lock_path.clone(),
            dry_run: options.dry_run,
        });
    }
    let (lock, changes) = locking::relock_with(env, &edit.project, index, previous.as_ref())?;
    for unresolved in &lock.unresolved {
        env.sink.emit(Event::SyncUnresolved {
            name: unresolved.name.clone(),
            required_by: unresolved.required_by.clone(),
        });
    }
    let plan = plan_sync(&lock, &install::list_installed(&dest.store())?, false);
    if lock_missing {
        announce_migration(env, &plan);
    }

    if options.dry_run {
        announce_edit(env, &edit, &lock, true);
        if !changes.is_empty() {
            env.sink.emit(Event::LockWouldChange { changes });
        }
        env.sink.emit(match plan.is_empty() {
            true => Event::SyncNothingToDo,
            false => Event::SyncPlanned {
                steps: plan.steps,
                dry_run: true,
            },
        });
        return Ok(());
    }

    let planned = fetch_locked(env, &edit.project, &plan.wanted)?;
    let lock_text = lock.to_toml()?;
    let sync_options = SyncOptions {
        install: options.clone(),
        ..SyncOptions::default()
    };
    let manifest = (edit.text != before).then_some((manifest_path.as_path(), edit.text.as_str()));
    restoring_on_failure(&[&manifest_path, &lock_path], || {
        write_manifest_and_lock(manifest, &lock_path, &lock_text)?;
        announce_edit(env, &edit, &lock, false);
        announce_written(env, lock_path.clone(), &lock, changes);
        match plan.is_empty() {
            true => env.sink.emit(Event::SyncNothingToDo),
            false => {
                let mut session = InstallSession::new(dest);
                let applied = apply_sync(env, &mut session, &lock, &plan, planned, &sync_options)?;
                finish(env, &mut session, &applied, options)?;
            }
        }
        Ok(())
    })
    .map_err(edit_error)
}

/// What a failure of the writes and the sync is. A sync error stays one; a
/// file that could not be put back adds a context ahead of the failure, and
/// that is the text the user must see, so such a failure is not unwrapped.
fn edit_error(failure: anyhow::Error) -> EditError {
    if failure.downcast_ref::<String>().is_some() {
        return EditError::Other(failure);
    }
    match failure.downcast::<SyncError>() {
        Ok(sync) => EditError::Sync(sync),
        Err(other) => EditError::Other(other),
    }
}

/// Say what the edit did to the manifest, and for a removal what keeps the
/// package locked.
fn announce_edit(env: &OpEnv, edit: &Edit, lock: &Lockfile, dry_run: bool) {
    let Some(change) = &edit.change else {
        return;
    };
    env.sink.emit(Event::ManifestEdited {
        edit: change.clone(),
        dev: edit.dev,
        dry_run,
    });
    if let ManifestEdit::Removed { name, .. } = change {
        let required_by: Vec<String> = lock
            .packages
            .iter()
            .filter(|package| {
                package
                    .requires
                    .iter()
                    .any(|required| required.eq_ignore_ascii_case(name))
            })
            .map(|package| package.name.clone())
            .collect();
        if !required_by.is_empty() {
            env.sink.emit(Event::PackageStillRequired {
                name: name.clone(),
                required_by,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::events::EventSink;
    use crate::target::Roots;
    use std::io::Write;
    use std::path::Path;
    use std::sync::Mutex;

    fn prepared(text: &str) -> Prepared {
        Prepared {
            found: (PathBuf::from("lvpm.toml"), project::parse(text).unwrap()),
            text: text.to_string(),
            lock_path: PathBuf::from("lvpm.lock"),
            previous: None,
            lock_missing: true,
        }
    }

    const MANIFEST: &str = "# mine\n[project]\nlabview = \"2024\"\n\n[dependencies]\n\
                            Beta = \">=1.0\" # keep\n\n[dev-dependencies]\ncaraya = \"*\"\n";

    #[test]
    fn a_request_is_a_name_and_a_constraint_that_is_written_when_it_is_given() {
        let parsed = |spec: &str| Request::parse(spec).unwrap();
        assert_eq!(
            parsed("gamma"),
            Request {
                name: "gamma".into(),
                constraint: None
            }
        );
        assert_eq!(parsed(" gamma@>=0.5 ").constraint.as_deref(), Some(">=0.5"));
        assert_eq!(parsed("gamma@1.0").constraint.as_deref(), Some("1.0"));
        assert_eq!(parsed("gamma@*").constraint.as_deref(), Some("*"));
    }

    #[test]
    fn nothing_after_the_at_sign_is_an_error_that_names_the_package() {
        // An unquoted `gamma@>=0.5` reaches the program as `gamma@`.
        for spec in ["gamma@", "gamma@ "] {
            assert!(
                matches!(Request::parse(spec), Err(EditError::EmptyConstraint { name }) if name == "gamma"),
                "{spec:?}"
            );
        }
    }

    #[test]
    fn no_name_and_a_constraint_lvpm_cannot_read_are_refused_before_anything_is_read() {
        for spec in ["", "@1.0", "gamma@<2.0", "gamma@~1.0"] {
            let why = Request::parse(spec).unwrap_err().to_string();
            assert!(
                why.contains("not a package name") || why.contains("unsupported version"),
                "{spec:?}: {why}"
            );
        }
    }

    #[test]
    fn a_new_entry_goes_at_the_end_of_its_table_and_nothing_else_changes() {
        let project = prepared(MANIFEST);
        let edit = project
            .set(DependencyTable::Dependencies, "gamma", ">=1.0")
            .unwrap();
        assert_eq!(
            edit.text,
            MANIFEST.replace("# keep\n", "# keep\ngamma = \">=1.0\"\n")
        );
        assert_eq!(
            edit.change,
            Some(ManifestEdit::Added {
                name: "gamma".into(),
                constraint: ">=1.0".into()
            })
        );
        assert!(!edit.dev);
        assert_eq!(edit.project.dependencies.len(), 2);
    }

    #[test]
    fn an_entry_that_is_there_is_updated_under_its_own_spelling() {
        let project = prepared(MANIFEST);
        let edit = project
            .set(DependencyTable::Dependencies, "beta", ">=0.5")
            .unwrap();
        assert_eq!(edit.text, MANIFEST.replace(">=1.0", ">=0.5"));
        assert_eq!(
            edit.change,
            Some(ManifestEdit::Updated {
                name: "Beta".into(),
                from: ">=1.0".into(),
                to: ">=0.5".into()
            })
        );
    }

    #[test]
    fn the_constraint_an_entry_already_has_is_no_change() {
        let project = prepared(MANIFEST);
        let edit = project
            .set(DependencyTable::Dependencies, "Beta", ">=1.0")
            .unwrap();
        assert_eq!(edit.text, MANIFEST);
        assert_eq!(edit.change, None);
    }

    #[test]
    fn a_dev_entry_goes_to_the_dev_table() {
        let project = prepared(MANIFEST);
        let edit = project
            .set(DependencyTable::DevDependencies, "delta", "1.0")
            .unwrap();
        assert!(edit.dev);
        assert_eq!(edit.project.dev_dependencies.len(), 2);
        assert_eq!(edit.project.dependencies.len(), 1);
    }

    #[test]
    fn a_removal_takes_only_the_entrys_line_and_names_it_as_the_manifest_does() {
        let project = prepared(MANIFEST);
        let edit = project
            .take_out(DependencyTable::Dependencies, "BETA")
            .unwrap();
        assert_eq!(edit.text, MANIFEST.replace("Beta = \">=1.0\" # keep\n", ""));
        assert_eq!(
            edit.change,
            Some(ManifestEdit::Removed {
                name: "Beta".into(),
                constraint: ">=1.0".into()
            })
        );
    }

    #[test]
    fn a_name_that_is_not_in_the_table_is_told_apart_from_one_in_the_other_table() {
        let project = prepared(MANIFEST);
        let refused = |table, name: &str| project.take_out(table, name).err().unwrap();
        assert!(matches!(
            refused(DependencyTable::Dependencies, "zeta"),
            EditError::NotADependency { name } if name == "zeta"
        ));
        let in_dev = refused(DependencyTable::Dependencies, "caraya");
        assert!(matches!(
            &in_dev,
            EditError::InOtherTable {
                found_in: DependencyTable::DevDependencies,
                ..
            }
        ));
        assert_eq!(in_dev.to_string(), "caraya is in [dev-dependencies]");
        let in_normal = refused(DependencyTable::DevDependencies, "beta");
        assert_eq!(in_normal.to_string(), "beta is in [dependencies]");
    }

    // ---- the whole operations, on real files in a scratch destination ----

    #[derive(Default)]
    struct Recorder(Mutex<Vec<Event>>);

    impl EventSink for Recorder {
        fn emit(&self, event: Event) {
            self.0.lock().unwrap().push(event);
        }
    }

    impl Recorder {
        fn events(&self) -> Vec<Event> {
            self.0.lock().unwrap().clone()
        }
    }

    /// `(name, version, requires)`: each is installed as `vi.lib/unit/<name>.txt`.
    type Fixture<'a> = (&'a str, &'a str, &'a str);

    const PACKAGES: &[Fixture<'static>] = &[
        ("alpha", "1.0", ""),
        ("alpha", "2.0", ""),
        ("beta", "1.0", "alpha>=1.0"),
        ("gamma", "0.5", ""),
        ("gamma", "1.0", ""),
    ];

    const PROJECT: &str = "[project]\nlabview = \"2024\"\n\n[sources]\ndefaults = false\n\n\
                           [dependencies]\nbeta = \">=1.0\"\n";

    fn payload_path(name: &str) -> String {
        format!("vi.lib/unit/{name}.txt")
    }

    fn write_vip(path: &Path, fixture: Fixture) {
        write_vip_for_labview(path, fixture, "20.0");
    }

    /// The package needs LabVIEW `lv_min` (in the index's units) or newer.
    fn write_vip_for_labview(path: &Path, (name, version, requires): Fixture, lv_min: &str) {
        let options = zip::write::SimpleFileOptions::default();
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let mut spec = format!("[Package]\nName=\"{name}\"\nVersion=\"{version}\"\n\n");
        if !requires.is_empty() {
            spec.push_str(&format!("[Dependencies]\nRequires=\"{requires}\"\n\n"));
        }
        spec.push_str(&format!(
            "[Platform]\nExclusive_LabVIEW_Version=\"LabVIEW>={lv_min}\"\n\n\
             [Files]\nNum File Groups=\"1\"\n\n[File Group 0]\nTarget Dir=\"<application>\"\n\
             Replace Mode=\"Always\"\nNum Files=1\nFile 0=\"{}\"\n",
            payload_path(name)
        ));
        writer.start_file("spec", options).unwrap();
        writer.write_all(spec.as_bytes()).unwrap();
        writer
            .start_file(format!("File Group 0/{}", payload_path(name)), options)
            .unwrap();
        writer
            .write_all(format!("{name} {version}\n").as_bytes())
            .unwrap();
        std::fs::write(path, writer.finish().unwrap().into_inner()).unwrap();
    }

    /// A project, a folder of packages beside it, and a scratch destination.
    struct World {
        root: PathBuf,
    }

    impl World {
        fn new(tag: &str, manifest: &str) -> World {
            let root =
                std::env::temp_dir().join(format!("lvpm-edit-test-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("packages")).unwrap();
            std::fs::write(root.join(project::FILE_NAME), manifest).unwrap();
            for package in PACKAGES {
                let file = format!("{}-{}.vip", package.0, package.1);
                write_vip(&root.join("packages").join(file), *package);
            }
            World { root }
        }

        fn env<'a>(&self, sink: &'a Recorder) -> OpEnv<'a> {
            let config = Config {
                cache: self.root.join("cache"),
                local_sources: vec![self.root.join("packages")],
                venv_dir: PathBuf::from(".lv-venv"),
            };
            let mut env = OpEnv::new(sink, config, Vec::new(), self.root.clone());
            env.flags.prefix = Some(self.root.join("dest"));
            env
        }

        fn manifest(&self) -> String {
            std::fs::read_to_string(self.root.join(project::FILE_NAME)).unwrap()
        }

        fn lock(&self) -> Option<Vec<u8>> {
            std::fs::read(self.root.join(lockfile::FILE_NAME)).ok()
        }

        fn locked(&self) -> Vec<String> {
            let lock = Lockfile::read(&self.root.join(lockfile::FILE_NAME)).unwrap();
            lock.packages
                .iter()
                .map(|package| format!("{} {}", package.name, package.version))
                .collect()
        }

        fn installed(&self) -> Vec<String> {
            let store = Roots::scratch(&self.root.join("dest"));
            install::list_installed(&store)
                .unwrap()
                .iter()
                .map(|package| format!("{} {}", package.name, package.version))
                .collect()
        }
    }

    impl Drop for World {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn run_add(world: &World, spec: &str, options: &InstallOptions) -> Result<(), EditError> {
        add(
            &world.env(&Recorder::default()),
            spec,
            DependencyTable::Dependencies,
            options,
        )
    }

    #[test]
    fn adding_without_a_constraint_writes_a_floor_at_the_locked_version_then_locks_and_syncs() {
        let world = World::new("add", PROJECT);
        let sink = Recorder::default();
        add(
            &world.env(&sink),
            "gamma",
            DependencyTable::Dependencies,
            &InstallOptions::default(),
        )
        .unwrap();
        assert_eq!(world.manifest(), format!("{PROJECT}gamma = \">=1.0\"\n"));
        let expected = ["alpha 2.0", "beta 1.0", "gamma 1.0"];
        assert_eq!(world.locked(), expected);
        assert_eq!(world.installed(), expected);
        let lock = Lockfile::read(&world.root.join(lockfile::FILE_NAME)).unwrap();
        assert_eq!(
            lock.staleness(&project::parse(&world.manifest()).unwrap()),
            None
        );
        assert!(sink.events().contains(&Event::ManifestEdited {
            edit: ManifestEdit::Added {
                name: "gamma".into(),
                constraint: ">=1.0".into()
            },
            dev: false,
            dry_run: false
        }));
    }

    #[test]
    fn the_floor_is_the_version_the_lock_holds_when_a_pin_keeps_it_below_the_newest() {
        let manifest = format!("{PROJECT}alpha = \"1.0\"\n");
        let world = World::new("pinned", &manifest);
        let packages = world.root.join("packages");
        write_vip(
            &packages.join("omega-1.0.vip"),
            ("omega", "1.0", "alpha>=1.0"),
        );
        write_vip(
            &packages.join("omega-2.0.vip"),
            ("omega", "2.0", "alpha>=2.0"),
        );
        // omega 2.0 is the newest there is, and cannot be had beside alpha 1.0.
        run_add(&world, "omega", &InstallOptions::default()).unwrap();
        assert_eq!(world.manifest(), format!("{manifest}omega = \">=1.0\"\n"));
        assert!(world.locked().contains(&"omega 1.0".to_string()));
    }

    #[test]
    fn a_package_no_source_lists_is_a_conflict_that_says_so_and_changes_nothing() {
        let world = World::new("unlisted", PROJECT);
        let why = run_add(&world, "zeta", &InstallOptions::default()).unwrap_err();
        assert!(
            matches!(&why, EditError::Lock(LockError::Conflict(conflict))
                if conflict.to_string().contains("zeta")),
            "{why:?}"
        );
        assert_eq!(world.manifest(), PROJECT);
        assert_eq!(world.lock(), None);
    }

    #[test]
    fn a_package_whose_every_version_needs_a_newer_labview_says_which_and_changes_nothing() {
        let world = World::new("gated", PROJECT);
        write_vip_for_labview(
            &world.root.join("packages").join("omega-1.0.vip"),
            ("omega", "1.0", ""),
            "26.0",
        );
        let why = run_add(&world, "omega", &InstallOptions::default()).unwrap_err();
        assert!(
            matches!(
                &why,
                EditError::Lock(LockError::NeedsNewerLabview { name, needs_year: 2026, project_year: 2024, .. })
                    if name == "omega"
            ),
            "{why:?}"
        );
        assert_eq!(world.manifest(), PROJECT);
        assert_eq!(world.lock(), None);
    }

    #[test]
    fn a_dry_run_reports_the_edit_the_lock_and_the_plan_and_writes_nothing() {
        let world = World::new("dry", PROJECT);
        let sink = Recorder::default();
        let options = InstallOptions {
            dry_run: true,
            ..InstallOptions::default()
        };
        add(
            &world.env(&sink),
            "gamma",
            DependencyTable::Dependencies,
            &options,
        )
        .unwrap();
        assert_eq!(world.manifest(), PROJECT);
        assert_eq!(world.lock(), None);
        assert!(!world.root.join("dest").exists());
        let events = sink.events();
        assert!(
            events
                .iter()
                .any(|event| matches!(event, Event::ManifestEdited { dry_run: true, .. })),
            "{events:?}"
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, Event::LockWouldChange { .. })),
            "{events:?}"
        );
        assert!(
            events.iter().any(|event| matches!(
                event,
                Event::SyncPlanned { dry_run: true, steps } if steps.len() == 3
            )),
            "{events:?}"
        );
    }

    #[test]
    fn a_failed_lock_changes_nothing_and_names_the_conflict() {
        let manifest = format!("{PROJECT}alpha = \"1.0\"\n");
        let world = World::new("conflict", &manifest);
        write_vip(
            &world.root.join("packages").join("omega-1.0.vip"),
            ("omega", "1.0", "alpha>=2.0"),
        );
        let why = run_add(&world, "omega", &InstallOptions::default()).unwrap_err();
        assert!(
            matches!(why, EditError::Lock(LockError::Conflict(_))),
            "{why:?}"
        );
        assert_eq!(world.manifest(), manifest);
        assert_eq!(world.lock(), None);
    }

    #[test]
    fn a_locked_version_is_not_pinned_to_other_bytes_when_the_lock_is_made_again() {
        let world = World::new("repin", PROJECT);
        run_add(&world, "gamma@0.5", &InstallOptions::default()).unwrap();
        let (manifest, lock) = (world.manifest(), world.lock().unwrap());

        // The feed now serves other bytes for alpha 2.0, and the cache is gone.
        let altered = world.root.join("packages/alpha-2.0.vip");
        write_vip_for_labview(&altered, ("alpha", "2.0", ""), "21.0");
        std::fs::remove_dir_all(world.root.join("cache")).unwrap();

        let why = run_add(&world, "gamma", &InstallOptions::default()).unwrap_err();
        let EditError::Lock(LockError::LockedArchives(problems)) = why else {
            panic!("expected archives that differ from the lock, got {why:?}");
        };
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(problems[0].name, "alpha");
        assert_eq!(
            problems[0].fault,
            crate::errors::ArchiveFault::DiffersFromLock {
                found_sha256: crate::cache::sha256_hex(&std::fs::read(&altered).unwrap())
            }
        );
        assert_eq!(world.manifest(), manifest);
        assert_eq!(world.lock().unwrap(), lock);
    }

    #[test]
    fn a_sync_that_fails_part_way_puts_the_manifest_and_the_lock_back() {
        let world = World::new("restore", PROJECT);
        run_add(&world, "alpha", &InstallOptions::default()).unwrap();
        let (manifest, lock) = (world.manifest(), world.lock().unwrap());

        // gamma's one file cannot be written: a folder is where it goes.
        let blocked = world.root.join("dest/LabVIEW").join(payload_path("gamma"));
        std::fs::create_dir_all(&blocked).unwrap();
        let why = run_add(&world, "gamma", &InstallOptions::default()).unwrap_err();

        assert!(
            matches!(&why, EditError::Sync(SyncError::Incomplete { left, .. })
                if left.iter().any(|step| step.name() == "gamma")),
            "{why:?}"
        );
        assert_eq!(world.manifest(), manifest);
        assert_eq!(world.lock().unwrap(), lock);
    }

    #[test]
    fn a_lock_that_did_not_exist_is_removed_again_when_the_sync_fails() {
        let world = World::new("restore-new", PROJECT);
        std::fs::create_dir_all(world.root.join("dest/LabVIEW").join(payload_path("gamma")))
            .unwrap();
        run_add(&world, "gamma", &InstallOptions::default()).unwrap_err();
        assert_eq!(world.manifest(), PROJECT);
        assert_eq!(world.lock(), None);
    }

    #[test]
    fn a_sync_failure_stays_a_sync_error_and_one_that_could_not_be_put_back_says_so_first() {
        let sync_failed = || anyhow::Error::new(SyncError::Archives(Vec::new()));
        assert!(matches!(
            edit_error(sync_failed()),
            EditError::Sync(SyncError::Archives(_))
        ));

        let world = World::new("stuck", PROJECT);
        let file = world.root.join("stuck");
        std::fs::write(&file, b"old").unwrap();
        // By the time the guard restores it, the file is a folder.
        let failure = restoring_on_failure(&[&file], || {
            std::fs::remove_file(&file)?;
            std::fs::create_dir(&file)?;
            Err::<(), _>(sync_failed())
        })
        .unwrap_err();
        let shown = edit_error(failure);
        assert!(matches!(shown, EditError::Other(_)), "{shown:?}");
        let text = format!("{shown:#}");
        assert!(text.starts_with("and could not put back "), "{text}");
    }

    #[test]
    fn removing_the_last_dependency_empties_the_lock_and_the_destination() {
        let world = World::new("remove-last", PROJECT);
        run_add(&world, "gamma", &InstallOptions::default()).unwrap();
        let sink = Recorder::default();
        let env = world.env(&sink);
        remove(
            &env,
            "gamma",
            DependencyTable::Dependencies,
            &InstallOptions::default(),
        )
        .unwrap();
        remove(
            &env,
            "beta",
            DependencyTable::Dependencies,
            &InstallOptions::default(),
        )
        .unwrap();
        assert_eq!(world.manifest(), PROJECT.replace("beta = \">=1.0\"\n", ""));
        assert_eq!(world.locked(), Vec::<String>::new());
        assert_eq!(world.installed(), Vec::<String>::new());
    }

    #[test]
    fn a_package_another_one_requires_stays_locked_installed_and_is_said_to_be_required() {
        let manifest = format!("{PROJECT}alpha = \">=1.0\"\n");
        let world = World::new("still", &manifest);
        run_add(&world, "alpha@>=1.0", &InstallOptions::default()).unwrap();
        let sink = Recorder::default();
        remove(
            &world.env(&sink),
            "alpha",
            DependencyTable::Dependencies,
            &InstallOptions::default(),
        )
        .unwrap();
        assert_eq!(world.manifest(), PROJECT);
        assert_eq!(world.locked(), ["alpha 2.0", "beta 1.0"]);
        assert_eq!(world.installed(), ["alpha 2.0", "beta 1.0"]);
        assert!(sink.events().contains(&Event::PackageStillRequired {
            name: "alpha".into(),
            required_by: vec!["beta".into()]
        }));
    }

    #[test]
    fn a_name_that_is_not_a_dependency_changes_nothing_and_syncs_nothing() {
        let world = World::new("not-a-dependency", PROJECT);
        let sink = Recorder::default();
        let why = remove(
            &world.env(&sink),
            "zeta",
            DependencyTable::Dependencies,
            &InstallOptions::default(),
        )
        .unwrap_err();
        assert!(matches!(why, EditError::NotADependency { .. }), "{why:?}");
        assert!(sink.events().is_empty(), "{:?}", sink.events());
        assert_eq!(world.manifest(), PROJECT);
        assert_eq!(world.lock(), None);
    }
}
