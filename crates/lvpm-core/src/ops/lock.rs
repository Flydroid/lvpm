//! `lock`: resolve the manifest with the solver and write `lvpm.lock`.
//!
//! Locking reads the manifest and the catalog and nothing else: not what is
//! installed, not where the project lives. The solver chooses the versions;
//! this operation then gets every chosen archive through the cache, records
//! its SHA-256, and writes the lock in one step, so a lock that is written is
//! complete and a failed run leaves the old one untouched.

use super::env::OpEnv;
use super::session::Downloads;
use crate::cache::sha256_hex;
use crate::errors::{ArchiveFailure, LockError};
use crate::events::{Event, LockChange};
use crate::index::{self, Entry, Index};
use crate::lockfile::{self, Inputs, LockedPackage, LockedUnresolved, Lockfile};
use crate::project::Project;
use crate::solve::{self, Resolution, SolveError};
use crate::version::Version;
use anyhow::Context;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

/// Resolve the project's manifest and write `lvpm.lock` beside it. The lock
/// comes back as it was written.
pub fn lock(env: &OpEnv) -> Result<Lockfile, LockError> {
    let found = env
        .manifest_for(None, None)?
        .ok_or_else(|| LockError::NoManifest {
            searched_from: env.cwd.clone(),
        })?;
    let (manifest_path, project) = &found;
    let labview = project.labview.as_deref().ok_or(LockError::NoLabview)?;
    let gate = lv_gate(labview)?;
    let lock_path = manifest_path.with_file_name(lockfile::FILE_NAME);
    // A lock that cannot be read is simply not a preference: it is rewritten.
    let previous = Lockfile::read(&lock_path).ok();

    let index = env.load_index(Some(&found))?;
    let resolution = resolve(env, project, &index, previous.as_ref(), gate)?;
    let hashes = hash_archives(env, &resolution)?;
    let feeds = index::feeds(&project.resolved_sources()?, project.default_sources);
    let lock = build(project, labview, &resolution, &hashes, &feeds);

    for unresolved in &resolution.unresolved {
        env.sink.emit(Event::RequirementUnresolved {
            name: unresolved.name.clone(),
            required_by: unresolved.required_by.clone(),
        });
    }
    write_atomically(&lock_path, &lock.to_toml()?)?;
    let changes = changes(previous.as_ref(), &lock);
    if !changes.is_empty() {
        env.sink.emit(Event::LockChanged { changes });
    }
    env.sink.emit(Event::LockWritten {
        path: lock_path,
        packages: lock.packages.len(),
    });
    Ok(lock)
}

/// The project's LabVIEW in the units of an index's `lv_min`: 24.0 for 2024.
fn lv_gate(labview: &str) -> Result<f64, LockError> {
    match labview.trim().parse::<u32>() {
        Ok(year) if year >= 2000 => Ok(f64::from(year - 2000)),
        _ => Err(LockError::LabviewNotAYear {
            value: labview.to_string(),
        }),
    }
}

/// The solver's answer for the manifest's direct and dev dependencies,
/// keeping the versions an earlier lock chose where they still fit.
fn resolve(
    env: &OpEnv,
    project: &Project,
    index: &Index,
    previous: Option<&Lockfile>,
    gate: f64,
) -> Result<Resolution, LockError> {
    let constraints: Vec<_> = project
        .dependencies
        .iter()
        .chain(&project.dev_dependencies)
        .cloned()
        .collect();
    let preferences: Vec<(String, Version)> = previous
        .iter()
        .flat_map(|lock| &lock.packages)
        .map(|package| (package.name.clone(), Version::parse(&package.version)))
        .collect();
    solve::solve(&constraints, index, &preferences, Some(gate), &env.cancel).map_err(
        |why| match why {
            SolveError::Conflict(conflict) => LockError::Conflict(conflict),
            SolveError::Cancelled => LockError::Cancelled,
        },
    )
}

/// The SHA-256 of every chosen archive, by lowercased name. Each archive
/// comes through the cache, so a cached one needs no network. Every archive
/// that cannot be had is collected, and they are reported together.
fn hash_archives(
    env: &OpEnv,
    resolution: &Resolution,
) -> Result<HashMap<String, String>, LockError> {
    let total = resolution.packages.len();
    // Nothing to fetch opens nothing: a manifest with no dependencies needs
    // no cache.
    let Some(downloads) = (total > 0).then(|| Downloads::open(env)).transpose()? else {
        return Ok(HashMap::new());
    };
    let mut hashes = HashMap::new();
    let mut failures = Vec::new();
    for (position, (key, entry)) in resolution.packages.iter().enumerate() {
        if env.cancel.is_cancelled() {
            return Err(LockError::Cancelled);
        }
        env.sink.emit(Event::LockHashing {
            name: entry.name.clone(),
            version: entry.version.to_string(),
            position: position + 1,
            total,
        });
        match downloads.fetch(entry) {
            Ok(bytes) => {
                hashes.insert(key.clone(), sha256_hex(&bytes));
            }
            Err(why) => failures.push(ArchiveFailure {
                name: entry.name.clone(),
                version: entry.version.to_string(),
                reason: format!("{why:#}"),
            }),
        }
    }
    match failures.is_empty() {
        true => Ok(hashes),
        false => Err(LockError::ArchivesUnavailable(failures)),
    }
}

/// The lock a resolution makes, given the hash of each archive in it.
fn build(
    project: &Project,
    labview: &str,
    resolution: &Resolution,
    hashes: &HashMap<String, String>,
    feeds: &[(String, String)],
) -> Lockfile {
    let direct = reachable_from_dependencies(project, resolution);
    let packages = resolution
        .packages
        .iter()
        .map(|(key, entry)| LockedPackage {
            name: entry.name.clone(),
            version: entry.version.to_string(),
            sha256: hashes[key].clone(),
            md5: entry.md5.clone(),
            source: index::source_of(&entry.url, feeds),
            requires: locked_requirements(entry, resolution),
            dev: !direct.contains(key),
        })
        .collect();
    let unresolved = resolution
        .unresolved
        .iter()
        .map(|unresolved| LockedUnresolved {
            name: unresolved.name.clone(),
            required_by: unresolved.required_by.clone(),
        })
        .collect();
    Lockfile {
        version: lockfile::FORMAT_VERSION,
        labview: labview.trim().to_string(),
        inputs: Inputs::of(project),
        packages,
        unresolved,
    }
}

/// The locked packages `entry` requires, by their locked names. A requirement
/// no source lists is not here: the lock records it as unresolved.
fn locked_requirements(entry: &Entry, resolution: &Resolution) -> Vec<String> {
    entry
        .requires
        .iter()
        .filter_map(|requirement| resolution.packages.get(&requirement.name.to_lowercase()))
        .map(|required| required.name.clone())
        .collect()
}

/// Every package, by lowercased name, that `[dependencies]` needs directly or
/// through other packages. What is left over is needed only for development.
fn reachable_from_dependencies(project: &Project, resolution: &Resolution) -> HashSet<String> {
    let mut reached = HashSet::new();
    let mut pending: Vec<String> = project
        .dependencies
        .iter()
        .map(|(name, _)| name.to_lowercase())
        .collect();
    while let Some(key) = pending.pop() {
        let Some(entry) = resolution.packages.get(&key) else {
            continue;
        };
        if reached.insert(key) {
            pending.extend(
                entry
                    .requires
                    .iter()
                    .map(|requirement| requirement.name.to_lowercase()),
            );
        }
    }
    reached
}

/// How `new` differs from `old`, by package name; every package is added when
/// there is no `old`.
fn changes(old: Option<&Lockfile>, new: &Lockfile) -> Vec<LockChange> {
    let versions = |lock: Option<&Lockfile>| -> BTreeMap<String, (String, String)> {
        lock.iter()
            .flat_map(|lock| &lock.packages)
            .map(|package| {
                (
                    package.name.to_lowercase(),
                    (package.name.clone(), package.version.clone()),
                )
            })
            .collect()
    };
    let (old, new) = (versions(old), versions(Some(new)));
    let keys: std::collections::BTreeSet<&String> = old.keys().chain(new.keys()).collect();
    keys.into_iter()
        .filter_map(|key| match (old.get(key), new.get(key)) {
            (None, Some((name, version))) => Some(LockChange::Added {
                name: name.clone(),
                version: version.clone(),
            }),
            (Some((name, version)), None) => Some(LockChange::Removed {
                name: name.clone(),
                version: version.clone(),
            }),
            (Some((_, from)), Some((name, to))) if from != to => Some(LockChange::Changed {
                name: name.clone(),
                from: from.clone(),
                to: to.clone(),
            }),
            _ => None,
        })
        .collect()
}

/// The new text goes beside the old file first and replaces it in one step:
/// whoever reads the lock sees the old one or the new one, never half of it.
fn write_atomically(path: &Path, text: &str) -> anyhow::Result<()> {
    let staged = path.with_extension("lock.tmp");
    std::fs::write(&staged, text).with_context(|| format!("writing {}", staged.display()))?;
    std::fs::rename(&staged, path).map_err(|why| {
        std::fs::remove_file(&staged).ok();
        anyhow::Error::new(why).context(format!("replacing {}", path.display()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::events::EventSink;
    use crate::index::Requirement;
    use crate::lockfile::LOCAL_SOURCE;
    use crate::solve::Unresolved;
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::Mutex;

    // ---- the pure parts ----

    fn entry(name: &str, version: &str, requires: &[&str], url: &str) -> Entry {
        Entry {
            name: name.to_string(),
            version: Version::parse(version),
            url: url.to_string(),
            md5: None,
            display_name: None,
            requires: requires
                .iter()
                .map(|required| Requirement {
                    name: required.to_string(),
                    min: None,
                    min_exclusive: false,
                })
                .collect(),
            lv_min: None,
        }
    }

    fn resolution(entries: Vec<Entry>, unresolved: Vec<Unresolved>) -> Resolution {
        Resolution {
            packages: entries
                .into_iter()
                .map(|entry| (entry.name.to_lowercase(), entry))
                .collect(),
            unresolved,
        }
    }

    fn manifest(text: &str) -> Project {
        crate::project::parse(text).unwrap()
    }

    fn hashes(resolution: &Resolution) -> HashMap<String, String> {
        resolution
            .packages
            .keys()
            .map(|key| (key.clone(), format!("sha-of-{key}")))
            .collect()
    }

    #[test]
    fn a_labview_year_is_the_index_unit_and_anything_else_is_refused() {
        assert_eq!(lv_gate("2024").unwrap(), 24.0);
        assert_eq!(lv_gate(" 2026 ").unwrap(), 26.0);
        for not_a_year in ["", "soon", "20.5", "1999"] {
            assert!(
                matches!(
                    lv_gate(not_a_year),
                    Err(LockError::LabviewNotAYear { value }) if value == not_a_year
                ),
                "{not_a_year:?}"
            );
        }
    }

    #[test]
    fn only_what_dev_dependencies_need_is_marked_dev() {
        let project = manifest(
            "[dependencies]\nBeta = \">=1.0\"\n\n[dev-dependencies]\ncaraya = \"*\"\nshared = \"*\"\n",
        );
        let resolution = resolution(
            vec![
                entry("alpha", "2.0", &[], "a"),
                entry("beta", "1.0", &["alpha", "Shared"], "b"),
                entry("caraya", "1.0", &["alpha", "helper"], "c"),
                entry("helper", "1.0", &[], "h"),
                entry("shared", "1.0", &[], "s"),
            ],
            Vec::new(),
        );
        let lock = build(&project, "2024", &resolution, &hashes(&resolution), &[]);
        let dev: Vec<(&str, bool)> = lock
            .packages
            .iter()
            .map(|package| (package.name.as_str(), package.dev))
            .collect();
        // alpha and shared are needed by `beta`, a direct dependency, even
        // though dev dependencies need them too; helper only by caraya.
        assert_eq!(
            dev,
            [
                ("alpha", false),
                ("beta", false),
                ("caraya", true),
                ("helper", true),
                ("shared", false)
            ]
        );
    }

    #[test]
    fn a_package_records_the_locked_packages_it_requires_and_the_unresolved_ones_apart() {
        let project = manifest("[dependencies]\nbeta = \"*\"\n");
        let resolution = resolution(
            vec![
                entry("alpha", "2.0", &[], "a"),
                entry("beta", "1.0", &["Alpha", "ni_lib_missing"], "b"),
            ],
            vec![Unresolved {
                name: "ni_lib_missing".into(),
                required_by: "beta".into(),
            }],
        );
        let lock = build(&project, "2024", &resolution, &hashes(&resolution), &[]);
        assert_eq!(lock.packages[1].requires, ["alpha"], "by the locked name");
        assert_eq!(
            lock.unresolved,
            [LockedUnresolved {
                name: "ni_lib_missing".into(),
                required_by: "beta".into()
            }]
        );
        assert_eq!(lock.packages[0].sha256, "sha-of-alpha");
        assert_eq!(lock.labview, "2024");
        assert_eq!(lock.version, lockfile::FORMAT_VERSION);
    }

    #[test]
    fn a_package_comes_from_its_feed_or_from_local() {
        let project = manifest("[dependencies]\nhosted = \"*\"\nfolder = \"*\"\n");
        let feeds = vec![(
            "http://h/feed/index.vipr".to_string(),
            "http://h/feed/".to_string(),
        )];
        let resolution = resolution(
            vec![
                entry(
                    "hosted",
                    "1.0",
                    &[],
                    "http://h/feed/packages/hosted-1.0.vip",
                ),
                entry("folder", "1.0", &[], "C:/packages/folder-1.0.vip"),
            ],
            Vec::new(),
        );
        let lock = build(&project, "2024", &resolution, &hashes(&resolution), &feeds);
        assert_eq!(lock.packages[0].source, LOCAL_SOURCE);
        assert_eq!(lock.packages[1].source, "http://h/feed/index.vipr");
    }

    fn lock_of(packages: &[(&str, &str)]) -> Lockfile {
        let mut lock = build(
            &manifest(""),
            "2024",
            &resolution(Vec::new(), Vec::new()),
            &HashMap::new(),
            &[],
        );
        lock.packages = packages
            .iter()
            .map(|(name, version)| LockedPackage {
                name: name.to_string(),
                version: version.to_string(),
                sha256: String::new(),
                md5: None,
                source: LOCAL_SOURCE.to_string(),
                requires: Vec::new(),
                dev: false,
            })
            .collect();
        lock
    }

    fn added(name: &str, version: &str) -> LockChange {
        LockChange::Added {
            name: name.into(),
            version: version.into(),
        }
    }

    #[test]
    fn every_package_is_added_when_there_was_no_lock() {
        let new = lock_of(&[("beta", "1.0"), ("alpha", "2.0")]);
        assert_eq!(
            changes(None, &new),
            [added("alpha", "2.0"), added("beta", "1.0")]
        );
    }

    #[test]
    fn a_new_lock_is_described_by_what_was_added_removed_and_moved() {
        let old = lock_of(&[("alpha", "1.0"), ("beta", "1.0"), ("gone", "3.0")]);
        let new = lock_of(&[("Alpha", "2.0"), ("beta", "1.0"), ("new", "0.1")]);
        assert_eq!(
            changes(Some(&old), &new),
            [
                LockChange::Changed {
                    name: "Alpha".into(),
                    from: "1.0".into(),
                    to: "2.0".into()
                },
                LockChange::Removed {
                    name: "gone".into(),
                    version: "3.0".into()
                },
                added("new", "0.1"),
            ]
        );
        assert_eq!(changes(Some(&new), &new), []);
    }

    // ---- the whole operation, on real files ----

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

    /// A project directory: a manifest, and a folder of local packages beside
    /// it. Removed when it goes out of scope.
    struct ProjectDir {
        root: PathBuf,
    }

    /// `(name, version, LabVIEW it needs, requirements)`.
    type Fixture<'a> = (&'a str, &'a str, &'a str, &'a str);

    fn write_vip(path: &Path, (name, version, lv, requires): Fixture) {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        writer
            .start_file("spec", zip::write::SimpleFileOptions::default())
            .unwrap();
        let mut spec = format!("[Package]\nName={name}\nVersion={version}\n");
        if !requires.is_empty() {
            spec.push_str(&format!("[Dependencies]\nRequires={requires}\n"));
        }
        spec.push_str(&format!(
            "[Platform]\nExclusive_LabVIEW_Version=LabVIEW>={lv}\n"
        ));
        writer.write_all(spec.as_bytes()).unwrap();
        std::fs::write(path, writer.finish().unwrap().into_inner()).unwrap();
    }

    impl ProjectDir {
        fn new(tag: &str, manifest: &str, packages: &[Fixture]) -> ProjectDir {
            let root =
                std::env::temp_dir().join(format!("lvpm-lock-test-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("packages")).unwrap();
            std::fs::write(root.join(crate::project::FILE_NAME), manifest).unwrap();
            for package in packages {
                let file = format!("{}-{}.vip", package.0, package.1);
                write_vip(&root.join("packages").join(file), *package);
            }
            ProjectDir { root }
        }

        fn env<'a>(&self, sink: &'a Recorder) -> OpEnv<'a> {
            let config = Config {
                cache: self.root.join("cache"),
                local_sources: vec![self.root.join("packages")],
                venv_dir: PathBuf::from(".lv-venv"),
            };
            OpEnv::new(sink, config, Vec::new(), self.root.clone())
        }

        fn lock_path(&self) -> PathBuf {
            self.root.join(lockfile::FILE_NAME)
        }
    }

    impl Drop for ProjectDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    const MANIFEST: &str = "[project]\nlabview = \"2023\"\n\n[sources]\ndefaults = false\n\n\
                            [dependencies]\ngamma = \"*\"\n";

    #[test]
    fn a_package_that_needs_a_newer_labview_than_the_projects_is_not_locked() {
        let project = ProjectDir::new(
            "gate",
            MANIFEST,
            &[("gamma", "1.0", "23.0", ""), ("gamma", "1.5", "24.0", "")],
        );
        let sink = Recorder::default();
        let lock = lock(&project.env(&sink)).unwrap();
        assert_eq!(lock.packages.len(), 1);
        assert_eq!(lock.packages[0].version, "1.0");
        assert_eq!(lock.labview, "2023");
        let written = std::fs::read_to_string(project.lock_path()).unwrap();
        assert_eq!(written, lock.to_toml().unwrap());
        assert!(!project.root.join("lvpm.lock.tmp").exists());
        let events = sink.events();
        assert!(
            matches!(
                &events[..],
                [
                    Event::LockHashing {
                        position: 1,
                        total: 1,
                        ..
                    },
                    Event::LockChanged { .. },
                    Event::LockWritten { packages: 1, .. },
                ]
            ),
            "{events:?}"
        );
    }

    #[test]
    fn a_failed_lock_leaves_the_old_one_as_it_was() {
        let project = ProjectDir::new("keep", MANIFEST, &[("gamma", "1.0", "23.0", "")]);
        let sink = Recorder::default();
        lock(&project.env(&sink)).unwrap();
        let before = std::fs::read(project.lock_path()).unwrap();

        std::fs::write(
            project.root.join(crate::project::FILE_NAME),
            MANIFEST.replace("gamma", "zeta"),
        )
        .unwrap();
        let why = lock(&project.env(&sink)).unwrap_err();
        assert!(matches!(why, LockError::Conflict(_)), "{why:?}");
        assert_eq!(std::fs::read(project.lock_path()).unwrap(), before);
    }

    #[test]
    fn locking_needs_a_project_and_a_labview_that_is_a_year() {
        let sink = Recorder::default();
        let bare = std::env::temp_dir().join(format!("lvpm-lock-test-bare-{}", std::process::id()));
        std::fs::create_dir_all(&bare).unwrap();
        let env = OpEnv::new(
            &sink,
            Config {
                cache: bare.join("cache"),
                local_sources: Vec::new(),
                venv_dir: PathBuf::from(".lv-venv"),
            },
            Vec::new(),
            bare.clone(),
        );
        assert!(
            matches!(lock(&env), Err(LockError::NoManifest { searched_from }) if searched_from == bare)
        );
        std::fs::remove_dir_all(&bare).ok();

        for (text, expected) in [
            ("[project]\nname = \"x\"\n", "no labview"),
            ("[project]\nlabview = \"soon\"\n", "not a year"),
        ] {
            let project = ProjectDir::new("manifest", text, &[]);
            let why = lock(&project.env(&sink)).unwrap_err().to_string();
            assert!(why.contains(expected), "{why}");
            assert!(!project.lock_path().exists());
        }
    }

    const BOTH_TABLES: &str = "[project]\nlabview = \"2023\"\n\n[sources]\ndefaults = false\n\n\
                               [dependencies]\ngamma = \">=1.0\"\n\n\
                               [dev-dependencies]\ncaraya = \"*\"\ngamma = \"1.5\"\n";
    const GAMMAS: &[Fixture<'static>] = &[
        ("gamma", "1.0", "23.0", ""),
        ("gamma", "1.5", "23.0", "helper>=1.0"),
        ("gamma", "2.0", "23.0", "helper>=1.0"),
        ("helper", "1.0", "23.0", ""),
        ("caraya", "1.0", "23.0", "helper>=1.0"),
    ];

    #[test]
    fn a_package_in_both_tables_is_locked_once_at_a_version_both_admit() {
        let project = ProjectDir::new("both", BOTH_TABLES, GAMMAS);
        let lock = lock(&project.env(&Recorder::default())).unwrap();
        let locked: Vec<(&str, &str)> = lock
            .packages
            .iter()
            .map(|package| (package.name.as_str(), package.version.as_str()))
            .collect();
        // `>=1.0` alone would take 2.0, and the dev line alone allows 1.5 only.
        assert_eq!(
            locked,
            [("caraya", "1.0"), ("gamma", "1.5"), ("helper", "1.0")]
        );
        // Both lines are recorded as the manifest has them.
        assert_eq!(lock.inputs.dependencies["gamma"], ">=1.0");
        assert_eq!(lock.inputs.dev_dependencies["gamma"], "1.5");
    }

    #[test]
    fn a_package_in_both_tables_and_what_only_it_needs_are_not_dev() {
        let project = ProjectDir::new("both-dev", BOTH_TABLES, GAMMAS);
        let lock = lock(&project.env(&Recorder::default())).unwrap();
        let dev: Vec<(&str, bool)> = lock
            .packages
            .iter()
            .map(|package| (package.name.as_str(), package.dev))
            .collect();
        // helper is needed by gamma (normal) as well as by caraya (dev).
        assert_eq!(dev, [("caraya", true), ("gamma", false), ("helper", false)]);
        let text = lock.to_toml().unwrap();
        assert_eq!(text.matches("dev = true").count(), 1, "{text}");
    }

    #[test]
    fn a_package_in_both_tables_with_lines_that_cannot_both_hold_is_a_conflict_naming_both() {
        let project = ProjectDir::new(
            "both-conflict",
            &BOTH_TABLES.replace(">=1.0", "1.0"),
            GAMMAS,
        );
        let why = lock(&project.env(&Recorder::default())).unwrap_err();
        let LockError::Conflict(conflict) = why else {
            panic!("expected a conflict, got {why:?}");
        };
        for wanted in ["= 1.0", "= 1.5"] {
            assert!(
                conflict.facts.contains(&crate::solve::Fact::Manifest {
                    package: "gamma".into(),
                    wanted: wanted.into()
                }),
                "{wanted} is not in:\n{conflict}"
            );
        }
        assert_eq!(conflict.facts.len(), 2, "{conflict}");
        assert!(!project.lock_path().exists());
    }

    #[test]
    fn a_cancelled_token_stops_locking_before_anything_is_written() {
        let project = ProjectDir::new("cancel", MANIFEST, &[("gamma", "1.0", "23.0", "")]);
        let sink = Recorder::default();
        let env = project.env(&sink);
        env.cancel.cancel();
        assert!(matches!(lock(&env), Err(LockError::Cancelled)));
        assert!(!project.lock_path().exists());
    }

    #[test]
    fn a_manifest_with_no_dependencies_locks_to_an_empty_lock() {
        let project = ProjectDir::new(
            "empty",
            "[project]\nlabview = \"2023\"\n\n[sources]\ndefaults = false\n",
            &[],
        );
        let sink = Recorder::default();
        let lock = lock(&project.env(&sink)).unwrap();
        assert!(lock.packages.is_empty());
        assert!(project.lock_path().is_file());
        assert!(
            !project.root.join("cache/content").exists(),
            "nothing was fetched"
        );
    }
}
