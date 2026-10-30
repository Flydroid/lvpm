//! `lvpm init`: a folder becomes a project.
//!
//! All checks come first and write nothing; then `lvpm.toml` is written once,
//! with `create_new`, so a manifest that appears after the check is refused too.

use super::env::OpEnv;
use crate::errors::InitError;
use crate::events::{AddedDependency, Event, Retarget, SourceKind};
use crate::git::{self, Runner};
use crate::manifest_edit::DependencyTable;
use crate::project::Constraint;
use crate::target::LvTarget;
use crate::venv::{self, Venv};
use crate::vipm_interop::Converted;
use crate::{index, manifest_edit, paths, project, vipc, vipm_interop};
use anyhow::anyhow;
use serde::Serialize;
use std::cmp::Reverse;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use toml_edit::{DocumentMut, Item, Table};

/// The version a new project starts at.
const FIRST_VERSION: &str = "0.1.0";

/// What `init` made.
#[derive(Debug, Clone, Serialize)]
pub struct Initialized {
    /// The `lvpm.toml` it wrote.
    pub manifest: PathBuf,
    /// The year its `labview` states, if it has one.
    pub labview: Option<u32>,
    /// The venv bound to that LabVIEW, or why there is none.
    pub venv: Result<Venv, VenvSkip>,
}

/// Why `init` made no venv. The folder is set up all the same.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VenvSkip {
    pub why: String,
}

/// What `--from` read: the one input `run` takes, whatever file made it.
#[derive(Debug, Clone, Default)]
pub struct Seed {
    pub name: Option<String>,
    pub version: Option<String>,
    /// The year of the LabVIEW the source file targets; reported, never used.
    pub source_labview: Option<u32>,
    /// `[dependencies]`: name, constraint as `lvpm.toml` writes it.
    pub deps: Vec<(String, String)>,
    /// `[dev-dependencies]`.
    pub dev_deps: Vec<(String, String)>,
    /// How many packages the source marks as another's dependency and so left out.
    pub skipped: usize,
    /// `[nipm.dependencies]` as the source wrote it, carried over unread.
    pub nipm: Option<Table>,
    /// The fields of the source that `lvpm.toml` has no use for, when its reader
    /// reports them.
    pub unused: Option<Vec<String>>,
    /// Packages the source file carries, to be imported into the cache.
    pub embedded: Vec<Embedded>,
    /// Packages the source file says it carries and does not: `(name, version)`.
    pub not_embedded: Vec<(String, String)>,
}

/// A package archive inside the `--from` file.
#[derive(Debug, Clone)]
pub struct Embedded {
    pub name: String,
    pub version: String,
    /// Where the cache says it came from, e.g. `p.vipc!/Packages/x-1.0.vip`.
    pub label: String,
    pub bytes: Vec<u8>,
}

/// The LabVIEW the manifest names, and how to choose another installation.
#[derive(Debug, PartialEq)]
struct Choice {
    year: Option<u32>,
    /// Where the installation of that year is, when it is installed.
    path: Option<PathBuf>,
    /// Every installation, newest first.
    installed: Vec<String>,
    retarget: Option<Retarget>,
}

/// `--labview-version`'s `flag` year, else the newest installation's. Of two
/// bitnesses of one year the 64-bit one wins, whatever order `targets` is in.
///
/// `retarget` is the first installation that is not the chosen one, so with a
/// flag year that is not installed it is the newest; it is `None` with only one
/// installation. A flag year that is not installed is still the year written.
fn choose_labview(targets: &[LvTarget], flag: Option<u32>) -> Choice {
    let mut ranked: Vec<&LvTarget> = targets.iter().collect();
    ranked.sort_by_key(|t| Reverse((t.year(), t.bitness)));
    let (mut chosen, mut other) = (None, None);
    for &target in &ranked {
        if chosen.is_none() && flag.is_none_or(|year| target.year() == year) {
            chosen = Some(target);
        } else if other.is_none() {
            other = Some(target);
        }
    }
    let year = flag.or(chosen.map(LvTarget::year));
    let retarget =
        other
            .filter(|_| ranked.len() > 1)
            .map(|other| match Some(other.year()) == year {
                true => Retarget::Path(other.path.clone()),
                false => Retarget::Year(other.year()),
            });
    Choice {
        year,
        path: chosen.map(|t| t.path.clone()),
        installed: ranked.iter().map(|t| installation_label(t)).collect(),
        retarget,
    }
}

/// How an installation is told apart in what `init` says: `2026 (64-bit)`.
fn installation_label(target: &LvTarget) -> String {
    format!("{} ({}-bit)", target.year(), target.bitness)
}

/// Write an `lvpm.toml` in the project root — `--project`, else the directory
/// the command ran in — named after its folder. `labview` is `--labview-version`
/// when given, else the newest LabVIEW installed, and left out when there is none.
/// `from` (`--from`, relative to the working directory) adds the packages that file
/// chose as dependencies; its own LabVIEW is reported and never used. Once the
/// manifest is written, the package archives `from` holds are imported into the
/// cache, `git` makes the repository (the op does not know how git is run) and a
/// venv is bound to the chosen LabVIEW when it can be.
pub fn run(env: &OpEnv, from: Option<&Path>, runner: Runner) -> Result<Initialized, InitError> {
    let root = match &env.flags.project {
        Some(dir) => paths::resolve(&env.cwd, dir),
        None => paths::clean(&env.cwd),
    };
    if !root.is_dir() {
        return Err(InitError::ProjectMissing { dir: root });
    }
    let manifest = root.join(project::FILE_NAME);
    if manifest.exists() {
        return Err(InitError::ManifestExists { path: manifest });
    }
    let flag = env
        .flags
        .labview_version
        .as_deref()
        .map(|value| {
            project::labview_year(value).map_err(|_| InitError::BadLabview {
                value: value.to_string(),
            })
        })
        .transpose()?;
    let (seed, source_kind) = match from {
        Some(from) => read_source(&env.cwd, from).map(|(seed, kind)| (seed, Some(kind)))?,
        None => (Seed::default(), None),
    };
    let choice = choose_labview(&env.targets, flag);

    let text = manifest_text(&root, choice.year, &seed)?;
    write_new(&manifest, &text)?;
    env.sink.emit(Event::InitManifestWritten {
        path: manifest.clone(),
        labview: choice.year,
        installed: choice.installed,
        retarget: choice.retarget,
        source_labview: seed.source_labview,
        source_kind,
    });
    if choice.year.is_none() {
        env.sink.emit(Event::InitNoLabview);
    }
    if let Some(from) = from {
        let from = from.display().to_string();
        env.sink.emit(Event::InitDependenciesAdded {
            from: from.clone(),
            added: seed
                .deps
                .into_iter()
                .map(|(name, constraint)| AddedDependency { name, constraint })
                .collect(),
            skipped: seed.skipped,
        });
        if let Some(unused) = seed.unused {
            // Init carries the dev-dependencies, so the converter's line about them holds.
            env.sink.emit(Event::VipmManifestConverted {
                path: PathBuf::from(&from),
                unused,
                labview: None,
                dev_dependencies: seed.dev_deps.len(),
            });
        }
        import_embedded(env, &from, seed.embedded, seed.not_embedded);
    }
    env.sink.emit(Event::InitGit {
        outcome: git::init(&root, runner),
    });
    let venv = make_venv(env, &root, choice.year, choice.path.as_deref());
    Ok(Initialized {
        manifest,
        labview: choice.year,
        venv,
    })
}

/// What the file `--from` names says, and what kind of file it is. The kind is
/// told from the file's name before the file is opened, so this `match` is where
/// another kind of file gets its reader. `from` is looked for from `cwd`; the
/// errors name it as it was typed.
fn read_source(cwd: &Path, from: &Path) -> Result<(Seed, SourceKind), InitError> {
    let file = from.display().to_string();
    let name = from
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    let path = paths::resolve(cwd, from);
    match name.as_str() {
        n if n.ends_with(".dragon") => Err(InitError::DragonFile { file }),
        _ if vipm_interop::is_toml(from) => read_vipm_toml(&path, &file)
            .map(|seed| (seed, SourceKind::VipmToml))
            .map_err(bad_source),
        _ => read_vipc(&path, &file)
            .map(|seed| (seed, SourceKind::Vipc))
            .map_err(bad_source),
    }
}

fn bad_source(why: anyhow::Error) -> InitError {
    InitError::BadSource {
        why: format!("{why:#}"),
    }
}

/// The `vipm.toml` at `path`. The text is read once: the converter takes the
/// project and its packages from it, and `[nipm.dependencies]`, which the
/// converter only names, is lifted from it as it was written.
fn read_vipm_toml(path: &Path, file: &str) -> anyhow::Result<Seed> {
    let text =
        std::fs::read_to_string(path).map_err(|why| anyhow!("{file}: cannot be read: {why}"))?;
    // The TOML parser's own messages end in a newline.
    let refused = |why: String| anyhow!("{file}: {}", why.trim_end());
    let Converted { project, unused } =
        vipm_interop::parse_toml(&text).map_err(|why| refused(format!("{why:#}")))?;
    let source_labview = project
        .labview
        .as_deref()
        .map(|value| {
            project::labview_year(value).map_err(|_| {
                anyhow!("{file}: [project] labview-version {value:?} is not a LabVIEW year")
            })
        })
        .transpose()?;
    let doc: DocumentMut = text
        .parse()
        .map_err(|why: toml_edit::TomlError| refused(why.to_string()))?;
    let exact = |entries: Vec<(String, Constraint)>| -> Vec<(String, String)> {
        entries
            .into_iter()
            .map(|(name, constraint)| (name, constraint_text(constraint)))
            .collect()
    };
    Ok(Seed {
        name: project.name,
        version: project.version,
        source_labview,
        deps: exact(project.dependencies),
        dev_deps: exact(project.dev_dependencies),
        nipm: doc
            .get("nipm")
            .and_then(|nipm| nipm.get("dependencies"))
            .and_then(|table| table.clone().into_table().ok()),
        unused: Some(unused),
        ..Seed::default()
    })
}

/// A constraint as `lvpm.toml` writes it.
fn constraint_text(constraint: Constraint) -> String {
    match constraint {
        Constraint::Exact(version) => version.raw,
        Constraint::AtLeast(version) => format!(">={}", version.raw),
        Constraint::Any => "*".to_string(),
    }
}

/// The `.vipc` at `path`, with the archives it holds. Taking them is part of
/// the check, so a bad one fails before anything is written.
fn read_vipc(path: &Path, file: &str) -> anyhow::Result<Seed> {
    let vipc = vipc::read(path, file)?;
    let mut seed = vipc.seed();
    vipc.embed(path, file, &mut seed)?;
    Ok(seed)
}

/// Put the archives the source carried into the cache, and say which it did not
/// carry. The manifest is written by now, so a failure is a warning, never the
/// run's end.
fn import_embedded(
    env: &OpEnv,
    from: &str,
    embedded: Vec<Embedded>,
    not_embedded: Vec<(String, String)>,
) {
    for Embedded {
        name,
        version,
        label,
        bytes,
    } in embedded
    {
        env.sink.emit(
            match index::cache_add_bytes(&env.config.cache, &label, &bytes) {
                Ok(_) => Event::InitPackageImported {
                    name,
                    version,
                    from: from.to_string(),
                },
                Err(why) => Event::InitPackageImportFailed {
                    name,
                    version,
                    why: format!("{why:#}"),
                },
            },
        );
    }
    for (name, version) in not_embedded {
        env.sink.emit(Event::InitPackageNotEmbedded {
            name,
            version,
            from: from.to_string(),
        });
    }
}

/// The text of the new `lvpm.toml`: the skeleton (named and versioned as the seed
/// says, else after the folder at the first version), then each of the seed's
/// dependencies and its `[nipm.dependencies]`.
fn manifest_text(root: &Path, year: Option<u32>, seed: &Seed) -> Result<String, InitError> {
    let name = seed
        .name
        .clone()
        .unwrap_or_else(|| manifest_edit::folder_name(root));
    let version = seed.version.as_deref().unwrap_or(FIRST_VERSION);
    let mut text = manifest_edit::skeleton(&name, Some(version), year);
    for (table, entries) in [
        (DependencyTable::Dependencies, &seed.deps),
        (DependencyTable::DevDependencies, &seed.dev_deps),
    ] {
        for (name, constraint) in entries {
            text = manifest_edit::set(&text, table, name, constraint)?;
        }
    }
    match &seed.nipm {
        Some(nipm) => with_nipm_dependencies(&text, nipm),
        None => Ok(text),
    }
}

/// `text` with `[nipm.dependencies]` as `dependencies` has it, at the end.
fn with_nipm_dependencies(text: &str, dependencies: &Table) -> Result<String, InitError> {
    let mut doc: DocumentMut = text.parse().map_err(anyhow::Error::from)?;
    let mut nipm = Table::new();
    nipm.set_implicit(true);
    nipm.insert("dependencies", Item::Table(dependencies.clone()));
    doc.insert("nipm", Item::Table(nipm));
    Ok(doc.to_string())
}

/// The venv for the LabVIEW `year` names, bound to the installation at `path`
/// (by path, so the one `init` chose and said so of is the one bound, never
/// another of that year). Whatever stops it is a skip, in `venv::create`'s own
/// words when it is what said no.
fn make_venv(
    env: &OpEnv,
    root: &Path,
    year: Option<u32>,
    path: Option<&Path>,
) -> Result<Venv, VenvSkip> {
    let skip = |why: String| VenvSkip { why };
    let Some(year) = year else {
        return Err(skip("no LabVIEW found".into()));
    };
    let Some(path) = path else {
        return Err(skip(format!("LabVIEW {year} is not installed")));
    };
    venv::create(
        root,
        Some(&path.to_string_lossy()),
        None,
        &env.config.venv_dir,
        &env.targets,
        env.sink,
    )
    .map_err(|why| skip(format!("{why:#}")))
}

/// `text` as the file at `path`, which must not be there.
fn write_new(path: &Path, text: &str) -> Result<(), InitError> {
    manifest_edit::create_file(path, text).map_err(|why| match why.kind() {
        ErrorKind::AlreadyExists => InitError::ManifestExists {
            path: path.to_path_buf(),
        },
        _ => InitError::Other(anyhow!(why).context(format!("writing {}", path.display()))),
    })
}

#[cfg(test)]
mod tests {
    use super::super::testing::{Recorded, env, env_on, scratch_dir};
    use super::*;
    use crate::git::{GitOutcome, fake_output};
    use std::io;
    use std::process::{Command, Output};

    /// What a fake git answers.
    type Answer = fn() -> io::Result<Output>;

    /// A git that makes a repository.
    fn git_ok(_: &mut Command) -> io::Result<Output> {
        fake_output(0, "")
    }

    /// A git that must not be run: a test of a refusal passes it.
    fn no_git(_: &mut Command) -> io::Result<Output> {
        panic!("git must not be run")
    }

    /// An empty folder named `name`, below a scratch directory of its own.
    fn folder(tag: &str, name: &str) -> (PathBuf, PathBuf) {
        let scratch = scratch_dir(tag);
        let root = scratch.join(name);
        std::fs::create_dir_all(&root).unwrap();
        (scratch, root)
    }

    fn lv(year: u32, bitness: u8) -> LvTarget {
        LvTarget {
            version: f64::from(year - 2000),
            bitness,
            path: PathBuf::from(format!("LabVIEW {year} {bitness}-bit")),
        }
    }

    /// What `init` says and writes in an empty folder where exactly `targets`
    /// are installed, run with `flag` as `--labview-version`.
    struct Ran {
        events: Vec<Event>,
        manifest: String,
        done: Initialized,
        scratch: PathBuf,
    }

    impl Ran {
        fn init(tag: &str, targets: Vec<LvTarget>, flag: Option<&str>) -> Ran {
            let (scratch, root) = folder(tag, "App");
            let sink = Recorded::default();
            let mut env = env_on(&sink, &root);
            env.targets = targets;
            env.flags.labview_version = flag.map(String::from);
            let done = run(&env, None, &git_ok).unwrap();
            Ran {
                events: sink.events(),
                manifest: std::fs::read_to_string(&done.manifest).unwrap(),
                done,
                scratch,
            }
        }

        fn root(&self) -> &Path {
            self.done.manifest.parent().unwrap()
        }

        /// The one `InitManifestWritten`'s `(labview, installed, retarget)`.
        fn written(&self) -> (Option<u32>, Vec<String>, Option<Retarget>) {
            match &self.events[0] {
                Event::InitManifestWritten {
                    labview,
                    installed,
                    retarget,
                    ..
                } => (*labview, installed.clone(), retarget.clone()),
                other => panic!("the first event is the manifest: {other:?}"),
            }
        }
    }

    impl Drop for Ran {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.scratch);
        }
    }

    #[test]
    fn init_writes_the_skeleton_named_after_its_folder_and_says_so() {
        let (scratch, root) = folder("init-writes", "Serializer");
        let sink = Recorded::default();
        let mut env = env_on(&sink, &root);
        env.flags.labview_version = Some("2023".into());

        let done = run(&env, None, &git_ok).unwrap();

        assert_eq!(done.manifest, root.join("lvpm.toml"));
        assert_eq!(done.labview, Some(2023));
        assert_eq!(
            std::fs::read_to_string(&done.manifest).unwrap(),
            "[project]\nname = \"Serializer\"\nversion = \"0.1.0\"\nlabview = \">=2023\"\n\n[dependencies]\n"
        );
        assert_eq!(
            sink.events(),
            [
                Event::InitManifestWritten {
                    path: done.manifest.clone(),
                    labview: Some(2023),
                    installed: vec!["2026 (64-bit)".into()],
                    retarget: None,
                    source_labview: None,
                    source_kind: None,
                },
                Event::InitGit {
                    outcome: GitOutcome::Initialized
                }
            ]
        );
        let _ = std::fs::remove_dir_all(&scratch);
    }

    /// The event `init` ended on, which is its git outcome.
    fn git_outcome(events: &[Event]) -> GitOutcome {
        match events.last() {
            Some(Event::InitGit { outcome }) => outcome.clone(),
            other => panic!("the last event is the git outcome: {other:?}"),
        }
    }

    #[test]
    fn git_is_run_once_in_the_root_after_the_manifest_is_written() {
        let (scratch, root) = folder("init-git-after", "App");
        let calls = std::cell::RefCell::new(Vec::new());
        let sink = Recorded::default();

        run(&env_on(&sink, &root), None, &|command: &mut Command| {
            calls.borrow_mut().push((
                command.get_current_dir().map(Path::to_path_buf),
                root.join("lvpm.toml").is_file(),
            ));
            fake_output(0, "")
        })
        .unwrap();

        assert_eq!(*calls.borrow(), [(Some(root.clone()), true)]);
        assert_eq!(git_outcome(&sink.events()), GitOutcome::Initialized);
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn git_missing_or_failing_is_reported_and_leaves_the_manifest_and_an_ok() {
        let answers: [(Answer, GitOutcome); 2] = [
            (
                || Err(io::Error::from(io::ErrorKind::NotFound)),
                GitOutcome::GitMissing,
            ),
            (
                || fake_output(128, "fatal: permission"),
                GitOutcome::Failed {
                    why: "fatal: permission".into(),
                },
            ),
        ];
        for (answer, outcome) in answers {
            let (scratch, root) = folder("init-git-warns", "App");
            let sink = Recorded::default();

            let done = run(&env_on(&sink, &root), None, &|_: &mut Command| answer()).unwrap();

            assert!(done.manifest.is_file(), "{outcome:?}");
            assert_eq!(git_outcome(&sink.events()), outcome);
            let _ = std::fs::remove_dir_all(&scratch);
        }
    }

    #[test]
    fn inside_a_work_tree_git_is_not_run_and_the_top_is_reported() {
        let (scratch, repo) = folder("init-git-inside", "Repo");
        std::fs::create_dir(repo.join(".git")).unwrap();
        let app = repo.join("app");
        std::fs::create_dir(&app).unwrap();
        let sink = Recorded::default();

        run(&env_on(&sink, &app), None, &no_git).unwrap();

        assert_eq!(
            git_outcome(&sink.events()),
            GitOutcome::InsideWorkTree { top: repo }
        );
        assert!(!app.join(".git").exists());
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn a_refusal_runs_no_git_and_makes_no_venv() {
        let (scratch, root) = folder("init-refusal-no-git", "Taken");
        std::fs::write(root.join("lvpm.toml"), "# mine\n").unwrap();
        let calls = std::cell::Cell::new(0);

        let why = run(&env(&root), None, &|_: &mut Command| {
            calls.set(calls.get() + 1);
            fake_output(0, "")
        })
        .unwrap_err();

        assert!(matches!(why, InitError::ManifestExists { .. }), "{why:?}");
        assert_eq!(calls.get(), 0);
        assert!(!root.join(".lv-venv").exists());
        assert!(!root.join(".git").exists());
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn the_venv_is_bound_to_the_chosen_labview_and_found_again_through_the_binding() {
        let targets = vec![lv(2026, 64), lv(2025, 64)];
        let ran = Ran::init("init-venv-bound", targets.clone(), None);

        let made = ran.done.venv.clone().unwrap();

        assert_eq!(made.target.path, lv(2026, 64).path);
        assert_eq!(made.dir, ran.root().join(".lv-venv"));
        let found = venv::load(ran.root(), Path::new(".lv-venv"), &targets).unwrap();
        assert_eq!(found.target.path, lv(2026, 64).path);
    }

    #[test]
    fn the_venv_binds_the_installation_init_chose_not_the_first_of_its_year() {
        let mut newer_32_bit = lv(2026, 32);
        newer_32_bit.version = 26.3;
        let ran = Ran::init("init-venv-by-path", vec![newer_32_bit, lv(2026, 64)], None);

        let made = ran.done.venv.clone().unwrap();

        assert_eq!(
            (made.target.bitness, made.target.path),
            (64, lv(2026, 64).path)
        );
    }

    #[test]
    fn a_venv_that_cannot_be_made_is_a_skip_with_its_reason_and_init_still_succeeds() {
        let cases = [
            (vec![lv(2023, 64)], None, "2024 Q1"),
            (
                vec![lv(2026, 64)],
                Some("2025"),
                "LabVIEW 2025 is not installed",
            ),
            (Vec::new(), None, "no LabVIEW found"),
        ];
        for (targets, flag, why) in cases {
            let ran = Ran::init("init-venv-skip", targets, flag);

            let skip = ran.done.venv.clone().unwrap_err();

            assert!(skip.why.contains(why), "{why:?} is not in {:?}", skip.why);
            assert!(!ran.root().join(".lv-venv").exists(), "{why}");
        }
    }

    #[test]
    fn a_venv_that_is_already_there_is_kept_and_init_says_so() {
        let (scratch, root) = folder("init-venv-there", "App");
        let binding = root.join(".lv-venv").join(".lvpm").join("venv.json");
        std::fs::create_dir_all(binding.parent().unwrap()).unwrap();
        std::fs::write(&binding, "{\"mine\":1}").unwrap();

        let done = run(&env(&root), None, &git_ok).unwrap();

        let skip = done.venv.unwrap_err();
        assert!(skip.why.contains("already has a venv"), "{}", skip.why);
        assert_eq!(std::fs::read_to_string(&binding).unwrap(), "{\"mine\":1}");
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn the_newest_is_chosen_and_the_next_year_is_offered() {
        let ran = Ran::init(
            "init-newest",
            vec![lv(2026, 64), lv(2025, 64), lv(2024, 64)],
            None,
        );

        assert_eq!(
            ran.written(),
            (
                Some(2026),
                vec![
                    "2026 (64-bit)".into(),
                    "2025 (64-bit)".into(),
                    "2024 (64-bit)".into()
                ],
                Some(Retarget::Year(2025))
            )
        );
        assert!(
            ran.manifest.contains("labview = \">=2026\""),
            "{}",
            ran.manifest
        );
        assert_eq!(ran.done.labview, Some(2026));
    }

    #[test]
    fn the_newest_is_chosen_whatever_order_the_installations_come_in() {
        let ran = Ran::init("init-order-in", vec![lv(2024, 64), lv(2026, 64)], None);

        assert_eq!(ran.written().0, Some(2026));
        assert_eq!(ran.written().2, Some(Retarget::Year(2024)));
    }

    #[test]
    fn a_labview_version_overrides_the_newest_and_offers_the_newest_instead() {
        let ran = Ran::init("init-flag", vec![lv(2026, 64), lv(2025, 64)], Some("2025"));

        assert_eq!(ran.written().0, Some(2025));
        assert!(
            ran.manifest.contains("labview = \">=2025\""),
            "{}",
            ran.manifest
        );
        assert_eq!(ran.written().2, Some(Retarget::Year(2026)));
    }

    #[test]
    fn the_oldest_installation_still_offers_another_when_chosen_by_flag() {
        let ran = Ran::init(
            "init-flag-oldest",
            vec![lv(2026, 64), lv(2025, 64), lv(2024, 64)],
            Some("2024"),
        );

        assert_eq!(ran.written().2, Some(Retarget::Year(2026)));
    }

    #[test]
    fn a_flag_year_that_is_not_installed_is_written_and_offers_nothing_with_one_installation() {
        let ran = Ran::init("init-flag-absent-one", vec![lv(2026, 64)], Some("2025"));

        assert_eq!(ran.written().0, Some(2025));
        assert!(
            ran.manifest.contains("labview = \">=2025\""),
            "{}",
            ran.manifest
        );
        assert_eq!(ran.written().2, None);
        assert!(
            !ran.events.contains(&Event::InitNoLabview),
            "no warning when the flag named one"
        );
    }

    #[test]
    fn a_flag_year_that_is_not_installed_offers_the_newest_of_several() {
        let ran = Ran::init(
            "init-flag-absent-two",
            vec![lv(2024, 64), lv(2026, 64)],
            Some("2025"),
        );

        assert_eq!(ran.written().0, Some(2025));
        assert_eq!(
            ran.written().1,
            vec!["2026 (64-bit)".to_string(), "2024 (64-bit)".to_string()]
        );
        assert_eq!(ran.written().2, Some(Retarget::Year(2026)));
    }

    #[test]
    fn one_installation_offers_nothing() {
        let ran = Ran::init("init-one", vec![lv(2026, 64)], None);

        assert_eq!(
            ran.written(),
            (Some(2026), vec!["2026 (64-bit)".into()], None)
        );
    }

    #[test]
    fn two_bitnesses_of_a_year_choose_the_64_bit_and_offer_the_other_by_path() {
        for targets in [
            vec![lv(2026, 32), lv(2026, 64)],
            vec![lv(2026, 64), lv(2026, 32)],
        ] {
            let ran = Ran::init("init-bitness", targets, None);

            assert_eq!(
                ran.written(),
                (
                    Some(2026),
                    vec!["2026 (64-bit)".into(), "2026 (32-bit)".into()],
                    Some(Retarget::Path(lv(2026, 32).path))
                )
            );
        }
    }

    #[test]
    fn a_64_bit_older_year_does_not_outrank_a_newer_32_bit_one() {
        let ran = Ran::init("init-year-first", vec![lv(2025, 64), lv(2026, 32)], None);

        assert_eq!(ran.written().0, Some(2026));
        assert_eq!(ran.written().2, Some(Retarget::Year(2025)));
    }

    #[test]
    fn with_no_labview_the_manifest_has_none_and_init_warns_and_succeeds() {
        let ran = Ran::init("init-no-labview", Vec::new(), None);

        assert_eq!(
            ran.manifest,
            "[project]\nname = \"App\"\nversion = \"0.1.0\"\n\n[dependencies]\n"
        );
        assert_eq!(ran.done.labview, None);
        assert_eq!(ran.written(), (None, Vec::new(), None));
        assert_eq!(
            ran.events[1..],
            [
                Event::InitNoLabview,
                Event::InitGit {
                    outcome: GitOutcome::Initialized
                }
            ]
        );
    }

    #[test]
    fn with_no_labview_a_flag_still_names_the_year_and_nothing_warns() {
        let ran = Ran::init("init-no-labview-flag", Vec::new(), Some("2023"));

        assert!(
            ran.manifest.contains("labview = \">=2023\""),
            "{}",
            ran.manifest
        );
        assert!(
            !ran.events.contains(&Event::InitNoLabview),
            "{:?}",
            ran.events
        );
    }

    #[test]
    fn project_names_the_folder_and_is_taken_from_the_working_directory() {
        let (scratch, root) = folder("init-project", "Elsewhere");
        let mut env = env(&scratch);
        env.flags.project = Some(PathBuf::from("Elsewhere"));

        let done = run(&env, None, &git_ok).unwrap();

        assert_eq!(done.manifest, root.join("lvpm.toml"));
        assert!(!scratch.join("lvpm.toml").exists());
        assert!(
            std::fs::read_to_string(done.manifest)
                .unwrap()
                .contains("name = \"Elsewhere\"")
        );
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn a_project_folder_that_is_not_there_is_refused_and_not_made() {
        let scratch = scratch_dir("init-missing");
        let mut env = env(&scratch);
        env.flags.project = Some(PathBuf::from("Missing"));

        let why = run(&env, None, &no_git).unwrap_err();

        assert!(
            matches!(&why, InitError::ProjectMissing { dir } if *dir == scratch.join("Missing")),
            "{why:?}"
        );
        assert!(!scratch.join("Missing").exists());
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn a_project_that_is_a_file_is_refused_like_a_missing_folder() {
        let (scratch, root) = folder("init-file", "Taken");
        std::fs::write(root.join("afile"), "x").unwrap();
        let mut env = env(&root);
        env.flags.project = Some(PathBuf::from("afile"));

        let why = run(&env, None, &no_git).unwrap_err();

        assert!(matches!(why, InitError::ProjectMissing { .. }), "{why:?}");
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn a_write_that_fails_for_another_reason_says_what_it_was_writing() {
        let scratch = scratch_dir("init-write-fails");
        let path = scratch.join("no-such-dir").join("lvpm.toml");

        let why = write_new(&path, "x").unwrap_err();

        assert!(matches!(&why, InitError::Other(_)), "{why:?}");
        let InitError::Other(cause) = why else { return };
        assert!(
            format!("{cause:#}").starts_with(&format!("writing {}", path.display())),
            "{cause:#}"
        );
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn an_existing_manifest_is_refused_untouched_with_nothing_said() {
        let (scratch, root) = folder("init-exists", "Taken");
        std::fs::write(root.join("lvpm.toml"), "# mine\n").unwrap();
        let sink = Recorded::default();

        let why = run(&env_on(&sink, &root), None, &no_git).unwrap_err();

        assert!(
            matches!(&why, InitError::ManifestExists { path } if *path == root.join("lvpm.toml")),
            "{why:?}"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("lvpm.toml")).unwrap(),
            "# mine\n"
        );
        assert!(sink.events().is_empty());
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn write_new_refuses_an_existing_file_untouched() {
        let (scratch, root) = folder("init-race", "Raced");
        let path = root.join("lvpm.toml");
        std::fs::write(&path, "# mine\n").unwrap();

        let why = write_new(&path, "[project]\n").unwrap_err();

        assert!(matches!(why, InitError::ManifestExists { .. }), "{why:?}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# mine\n");
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn a_manifest_in_a_parent_folder_does_not_block() {
        let (scratch, root) = folder("init-parent", "Parent");
        std::fs::write(root.join("lvpm.toml"), "# the parent's\n").unwrap();
        let app = root.join("app");
        std::fs::create_dir_all(&app).unwrap();

        let done = run(&env_on(&Recorded::default(), &app), None, &git_ok).unwrap();

        assert_eq!(done.manifest, app.join("lvpm.toml"));
        assert_eq!(
            std::fs::read_to_string(root.join("lvpm.toml")).unwrap(),
            "# the parent's\n"
        );
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn a_labview_version_that_is_not_a_year_writes_nothing() {
        for value in ["abc", "26.3", "99", ""] {
            let (scratch, root) = folder("init-bad-year", "App");
            let mut env = env(&root);
            env.flags.labview_version = Some(value.into());

            let why = run(&env, None, &no_git).unwrap_err();

            assert!(
                matches!(&why, InitError::BadLabview { value: v } if v == value),
                "{value:?}: {why:?}"
            );
            assert!(!root.join("lvpm.toml").exists(), "{value:?}");
            let _ = std::fs::remove_dir_all(&scratch);
        }
    }

    #[test]
    fn a_manifest_or_a_missing_folder_is_reported_before_a_bad_flag() {
        let (scratch, root) = folder("init-order", "Taken");
        std::fs::write(root.join("lvpm.toml"), "").unwrap();
        let mut env = env(&root);
        env.flags.labview_version = Some("abc".into());
        assert!(matches!(
            run(&env, None, &no_git).unwrap_err(),
            InitError::ManifestExists { .. }
        ));

        env.flags.project = Some(PathBuf::from("Missing"));
        assert!(matches!(
            run(&env, None, &no_git).unwrap_err(),
            InitError::ProjectMissing { .. }
        ));
        let _ = std::fs::remove_dir_all(&scratch);
    }

    /// `name` in `dir`: a `.vipc` for LabVIEW `target` listing `packages` as
    /// `(name-version, pinned, dependency)`.
    fn vipc_in(dir: &Path, name: &str, target: &str, packages: &[(&str, bool, bool)]) -> PathBuf {
        use std::io::Write;
        let listed: String = packages
            .iter()
            .map(|(full, pinned, dependency)| {
                format!(
                    "<Package><Name>{full}</Name><Included>false</Included>\
                     <Dependency>{dependency}</Dependency><Pinned>{pinned}</Pinned></Package>"
                )
            })
            .collect();
        let xml = format!(
            "<VI_Package_Configuration><Target><Name>LabVIEW</Name>\
             <Version>{target}</Version>{listed}</Target></VI_Package_Configuration>"
        );
        let path = dir.join(name);
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        zip.start_file("config.xml", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(xml.as_bytes()).unwrap();
        zip.finish().unwrap();
        path
    }

    /// `p.vipc` in `dir` listing `listed` as `(name-version, included)`, chosen
    /// and not pinned, and holding each of `archives` as `(entry, bytes)`.
    fn vipc_holding(dir: &Path, listed: &[(&str, bool)], archives: &[(&str, &[u8])]) -> PathBuf {
        use std::io::Write;
        let packages: String = listed
            .iter()
            .map(|(full, included)| {
                format!(
                    "<Package><Name>{full}</Name><Included>{included}</Included>\
                     <Dependency>false</Dependency><Pinned>false</Pinned></Package>"
                )
            })
            .collect();
        let xml = format!(
            "<VI_Package_Configuration><Target><Name>LabVIEW</Name>\
             <Version>13.0</Version>{packages}</Target></VI_Package_Configuration>"
        );
        let path = dir.join("p.vipc");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("config.xml", options).unwrap();
        zip.write_all(xml.as_bytes()).unwrap();
        for (entry, bytes) in archives {
            zip.start_file(*entry, options).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
        path
    }

    /// The bytes of a package archive `demo_lib-1.0.0.vip`.
    fn demo_lib_archive(scratch: &Path) -> Vec<u8> {
        let archive = super::super::testing::vip_in(scratch, "demo_lib", "1.0.0", "", &[], false);
        std::fs::read(archive).unwrap()
    }

    /// An environment in `root` whose cache is `scratch/cache`.
    fn env_caching<'a>(sink: &'a Recorded, scratch: &Path, root: &Path) -> OpEnv<'a> {
        let mut env = env_on(sink, root);
        env.config.cache = scratch.join("cache");
        env
    }

    #[test]
    fn an_embedded_archive_is_imported_into_the_cache_after_the_manifest_and_before_git() {
        let (scratch, root) = folder("init-embed-import", "App");
        let archive = demo_lib_archive(&scratch);
        vipc_holding(
            &root,
            &[("demo_lib-1.0.0", true)],
            &[("Packages/demo_lib-1.0.0.vip", &archive)],
        );
        let sink = Recorded::default();
        let env = env_caching(&sink, &scratch, &root);

        let done = run(&env, Some(Path::new("p.vipc")), &git_ok).unwrap();

        assert!(
            std::fs::read_to_string(done.manifest)
                .unwrap()
                .contains("demo_lib = \">=1.0.0\"")
        );
        let events = sink.events();
        assert!(matches!(events[0], Event::InitManifestWritten { .. }));
        assert!(matches!(events[1], Event::InitDependenciesAdded { .. }));
        assert_eq!(
            events[2],
            Event::InitPackageImported {
                name: "demo_lib".into(),
                version: "1.0.0".into(),
                from: "p.vipc".into(),
            }
        );
        assert_eq!(events.len(), 4, "{events:?}");
        assert!(matches!(events[3], Event::InitGit { .. }));
        let cache = crate::cache::Cache::open(&scratch.join("cache")).unwrap();
        let imported = cache.imported_packages().unwrap();
        assert_eq!(
            imported
                .iter()
                .map(|p| (p.entry.name.as_str(), p.entry.version.as_str()))
                .collect::<Vec<_>>(),
            [("demo_lib", "1.0.0")]
        );
        drop(cache);
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn an_included_package_with_no_archive_inside_is_a_warning_and_still_a_dependency() {
        let (scratch, root) = folder("init-embed-missing", "App");
        vipc_holding(&root, &[("demo_lib-1.0.0", true)], &[]);
        let sink = Recorded::default();
        let env = env_caching(&sink, &scratch, &root);

        let done = run(&env, Some(Path::new("p.vipc")), &git_ok).unwrap();

        assert!(
            std::fs::read_to_string(done.manifest)
                .unwrap()
                .contains("demo_lib = \">=1.0.0\"")
        );
        assert!(sink.events().contains(&Event::InitPackageNotEmbedded {
            name: "demo_lib".into(),
            version: "1.0.0".into(),
            from: "p.vipc".into(),
        }));
        assert!(!scratch.join("cache").exists());
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn an_archive_that_cannot_be_taken_is_refused_and_nothing_is_written() {
        // Entries holding a real archive or junk, and what the refusal says.
        let cases = [
            (
                "not-a-package",
                vec!["demo_lib-1.0.0.vip"],
                false,
                "p.vipc: embedded demo_lib-1.0.0.vip is not a package: ",
            ),
            (
                "twice",
                vec!["a/demo_lib-1.0.0.vip", "b/demo_lib-1.0.0.vip"],
                true,
                "p.vipc: more than one demo_lib-1.0.0.vip inside",
            ),
        ];
        for (tag, entries, is_package, message) in cases {
            let (scratch, root) = folder(&format!("init-embed-refused-{tag}"), "App");
            let bytes = match is_package {
                true => demo_lib_archive(&scratch),
                false => b"junk".to_vec(),
            };
            let archives: Vec<(&str, &[u8])> = entries.iter().map(|e| (*e, &bytes[..])).collect();
            vipc_holding(&root, &[("demo_lib-1.0.0", true)], &archives);
            let sink = Recorded::default();
            let env = env_caching(&sink, &scratch, &root);

            let why = run(&env, Some(Path::new("p.vipc")), &no_git).unwrap_err();

            let InitError::BadSource { why } = why else {
                panic!("{tag}: {why:?}")
            };
            assert!(why.starts_with(message), "{tag}: {why}");
            assert!(sink.events().is_empty(), "{tag}");
            assert_nothing_written(&root);
            assert!(!scratch.join("cache").exists(), "{tag}");
            let _ = std::fs::remove_dir_all(&scratch);
        }
    }

    #[test]
    fn an_import_that_fails_after_the_write_is_a_warning_and_the_run_still_succeeds() {
        let (scratch, root) = folder("init-embed-io-fails", "App");
        let archive = demo_lib_archive(&scratch);
        vipc_holding(
            &root,
            &[("demo_lib-1.0.0", true)],
            &[("demo_lib-1.0.0.vip", &archive)],
        );
        std::fs::write(scratch.join("cache"), "a file where the cache folder goes").unwrap();
        let sink = Recorded::default();
        let env = env_caching(&sink, &scratch, &root);

        let done = run(&env, Some(Path::new("p.vipc")), &git_ok).unwrap();

        assert!(
            std::fs::read_to_string(done.manifest)
                .unwrap()
                .contains("demo_lib = \">=1.0.0\"")
        );
        let events = sink.events();
        let failed: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                Event::InitPackageImportFailed { name, version, why } => Some((name, version, why)),
                _ => None,
            })
            .collect();
        assert_eq!(failed.len(), 1, "{events:?}");
        assert_eq!(
            (failed[0].0.as_str(), failed[0].1.as_str()),
            ("demo_lib", "1.0.0")
        );
        assert!(!failed[0].2.is_empty());
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, Event::InitPackageImported { .. }))
        );
        assert!(
            matches!(events.last(), Some(Event::InitGit { .. })),
            "git still runs: {events:?}"
        );
        let _ = std::fs::remove_dir_all(&scratch);
    }

    /// A refusal leaves none of what `init` makes.
    fn assert_nothing_written(root: &Path) {
        for name in ["lvpm.toml", ".git", ".lv-venv"] {
            assert!(!root.join(name).exists(), "{name} was made");
        }
    }

    #[test]
    fn from_adds_the_chosen_packages_exact_or_floored_and_reports_them_and_the_skips() {
        let (scratch, root) = folder("init-from-adds", "App");
        vipc_in(
            &root,
            "p.vipc",
            "13.0",
            &[
                ("pinned_lib-1.0.0.1", true, false),
                ("floating_lib-2.0.0.2", false, false),
                ("needed_lib-3.0.0.3", false, true),
            ],
        );
        let sink = Recorded::default();
        let mut env = env_on(&sink, &root);
        env.flags.labview_version = Some("2023".into());

        let done = run(&env, Some(Path::new("p.vipc")), &git_ok).unwrap();

        assert_eq!(
            std::fs::read_to_string(&done.manifest).unwrap(),
            "[project]\nname = \"App\"\nversion = \"0.1.0\"\nlabview = \">=2023\"\n\n\
             [dependencies]\npinned_lib = \"1.0.0.1\"\nfloating_lib = \">=2.0.0.2\"\n"
        );
        assert_eq!(
            sink.events(),
            [
                Event::InitManifestWritten {
                    path: done.manifest.clone(),
                    labview: Some(2023),
                    installed: vec!["2026 (64-bit)".into()],
                    retarget: None,
                    source_labview: Some(2013),
                    source_kind: Some(SourceKind::Vipc),
                },
                Event::InitDependenciesAdded {
                    from: "p.vipc".into(),
                    added: vec![
                        AddedDependency {
                            name: "pinned_lib".into(),
                            constraint: "1.0.0.1".into()
                        },
                        AddedDependency {
                            name: "floating_lib".into(),
                            constraint: ">=2.0.0.2".into()
                        },
                    ],
                    skipped: 1,
                },
                Event::InitGit {
                    outcome: GitOutcome::Initialized
                }
            ]
        );
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn a_vipc_with_no_packages_adds_none_and_says_so() {
        let (scratch, root) = folder("init-from-empty", "App");
        vipc_in(&root, "p.vipc", "13.0", &[]);
        let sink = Recorded::default();

        let done = run(&env_on(&sink, &root), Some(Path::new("p.vipc")), &git_ok).unwrap();

        assert!(
            std::fs::read_to_string(done.manifest)
                .unwrap()
                .ends_with("[dependencies]\n")
        );
        assert!(sink.events().contains(&Event::InitDependenciesAdded {
            from: "p.vipc".into(),
            added: Vec::new(),
            skipped: 0
        }));
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn from_is_taken_from_the_working_directory_not_from_project() {
        let (scratch, work) = folder("init-from-cwd", "Work");
        let app = work.join("App");
        std::fs::create_dir(&app).unwrap();
        vipc_in(
            &work,
            "p.vipc",
            "13.0",
            &[("demo_lib-1.0.0.1", false, false)],
        );
        let mut env = env(&work);
        env.flags.project = Some(PathBuf::from("App"));

        let done = run(&env, Some(Path::new("p.vipc")), &git_ok).unwrap();

        assert_eq!(done.manifest, app.join("lvpm.toml"));
        assert!(
            std::fs::read_to_string(done.manifest)
                .unwrap()
                .contains("demo_lib = \">=1.0.0.1\"")
        );
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn the_labview_of_the_from_file_is_reported_and_never_chosen() {
        // The .vipc targets 2013, which is neither installed nor named by a flag.
        let ran = |tag, targets, flag| {
            let (scratch, root) = folder(tag, "App");
            vipc_in(&root, "p.vipc", "13.0", &[]);
            let sink = Recorded::default();
            let mut env = env_on(&sink, &root);
            env.targets = targets;
            env.flags.labview_version = flag;
            let done = run(&env, Some(Path::new("p.vipc")), &git_ok).unwrap();
            let manifest = std::fs::read_to_string(done.manifest).unwrap();
            let _ = std::fs::remove_dir_all(&scratch);
            (done.labview, manifest, sink.events())
        };

        let (labview, _, events) = ran("init-from-lv-installed", vec![lv(2026, 64)], None);
        assert_eq!(labview, Some(2026));
        assert!(matches!(
            events[0],
            Event::InitManifestWritten {
                labview: Some(2026),
                source_labview: Some(2013),
                ..
            }
        ));

        let (labview, manifest, events) = ran("init-from-lv-none", Vec::new(), None);
        assert_eq!(labview, None);
        assert!(!manifest.contains("labview"), "{manifest}");
        assert!(events.contains(&Event::InitNoLabview));
        assert!(matches!(
            events[0],
            Event::InitManifestWritten {
                labview: None,
                source_labview: Some(2013),
                ..
            }
        ));

        let (labview, _, _) = ran("init-from-lv-flag", Vec::new(), Some("2023".into()));
        assert_eq!(labview, Some(2023));
    }

    #[test]
    fn a_dragon_file_is_refused_by_its_name_alone_and_nothing_is_written() {
        for typed in ["p.dragon", "P.DRAGON", "sub/Project.Dragon", ".dragon"] {
            let (scratch, root) = folder("init-dragon", "App");
            let sink = Recorded::default();

            let why = run(&env_on(&sink, &root), Some(Path::new(typed)), &no_git).unwrap_err();

            assert!(
                matches!(&why, InitError::DragonFile { file } if file == typed),
                "{typed}: {why:?}"
            );
            assert!(sink.events().is_empty(), "{typed}");
            assert_nothing_written(&root);
            let _ = std::fs::remove_dir_all(&scratch);
        }
    }

    #[test]
    fn a_bad_from_file_is_refused_naming_it_as_typed_and_nothing_is_written() {
        let (scratch, root) = folder("init-from-bad", "App");
        std::fs::create_dir(root.join("sub")).unwrap();
        std::fs::write(root.join("sub").join("bad.vipc"), "hello").unwrap();
        let typed = Path::new("sub").join("bad.vipc");
        let missing = Path::new("sub").join("missing.vipc");
        let sink = Recorded::default();

        for (from, what) in [(&typed, "not a zip file"), (&missing, "cannot be read: ")] {
            let why = run(&env_on(&sink, &root), Some(from), &no_git).unwrap_err();

            let InitError::BadSource { why } = why else {
                panic!("{from:?}: {why:?}")
            };
            assert!(
                why.starts_with(&format!("{}: {what}", from.display())),
                "{why}"
            );
        }
        assert!(sink.events().is_empty());
        assert_nothing_written(&root);
        let _ = std::fs::remove_dir_all(&scratch);
    }

    /// A `vipm.toml` with every table the converter knows, one it does not use.
    const VIPM_TOML: &str = r#"[project]
name = "daq-tool"
version = "0.1.0"
labview-version = "2021"

[dependencies]
labview_open_source_lib_serializer = "1.2.1.16"

[dev-dependencies]
jki_vi_tester = "3.0.0.1"

[nipm.dependencies]
ni-daqmx-labview-support = "26.0"

[tool.vipm]
build = "x"
"#;

    /// `vipm.toml` holding `text` in a folder `name`, and `init --from` it run there.
    fn init_from_vipm_toml(
        tag: &str,
        name: &str,
        text: &str,
    ) -> (Result<Initialized, InitError>, Vec<Event>, PathBuf) {
        let (scratch, root) = folder(tag, name);
        std::fs::write(root.join("vipm.toml"), text).unwrap();
        let sink = Recorded::default();
        let mut env = env_on(&sink, &root);
        env.flags.labview_version = Some("2023".into());
        let done = run(&env, Some(Path::new("vipm.toml")), &git_ok);
        (done, sink.events(), scratch)
    }

    #[test]
    fn a_vipm_toml_becomes_the_lvpm_toml_with_its_project_packages_and_nipm_table() {
        let (done, events, scratch) = init_from_vipm_toml("init-vipm-toml", "App", VIPM_TOML);

        let done = done.unwrap();

        assert_eq!(
            std::fs::read_to_string(&done.manifest).unwrap(),
            "[project]\nname = \"daq-tool\"\nversion = \"0.1.0\"\nlabview = \">=2023\"\n\n\
             [dependencies]\nlabview_open_source_lib_serializer = \"1.2.1.16\"\n\n\
             [dev-dependencies]\njki_vi_tester = \"3.0.0.1\"\n\n\
             [nipm.dependencies]\nni-daqmx-labview-support = \"26.0\"\n"
        );
        assert_eq!(
            events[..3],
            [
                Event::InitManifestWritten {
                    path: done.manifest.clone(),
                    labview: Some(2023),
                    installed: vec!["2026 (64-bit)".into()],
                    retarget: None,
                    source_labview: Some(2021),
                    source_kind: Some(SourceKind::VipmToml),
                },
                Event::InitDependenciesAdded {
                    from: "vipm.toml".into(),
                    added: vec![AddedDependency {
                        name: "labview_open_source_lib_serializer".into(),
                        constraint: "1.2.1.16".into()
                    }],
                    skipped: 0,
                },
                Event::VipmManifestConverted {
                    path: PathBuf::from("vipm.toml"),
                    unused: vec!["tool".into()],
                    labview: None,
                    dev_dependencies: 1,
                },
            ]
        );
        assert_eq!(
            std::fs::read_to_string(done.manifest.with_file_name("vipm.toml")).unwrap(),
            VIPM_TOML
        );
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn a_vipm_toml_with_only_dependencies_is_named_after_the_folder_with_no_nipm_table() {
        let (done, events, scratch) = init_from_vipm_toml(
            "init-vipm-toml-deps",
            "tool",
            "[dependencies]\na_lib = \"1.0.0.1\"\n",
        );

        let manifest = std::fs::read_to_string(done.unwrap().manifest).unwrap();

        assert_eq!(
            manifest,
            "[project]\nname = \"tool\"\nversion = \"0.1.0\"\nlabview = \">=2023\"\n\n\
             [dependencies]\na_lib = \"1.0.0.1\"\n"
        );
        assert!(matches!(
            events[0],
            Event::InitManifestWritten {
                source_labview: None,
                ..
            }
        ));
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn the_nipm_table_is_copied_as_written_and_never_named_as_not_used() {
        let nipm = "[nipm.dependencies]\nni-daqmx = { version = \"24.5.0\", feed = \"ni-daq\" }\n\
                    ni-visa = '24.0'\n";
        let (done, events, scratch) = init_from_vipm_toml(
            "init-vipm-toml-nipm",
            "App",
            &format!("[nipm.feeds]\nni-daq = \"https://x\"\n\n{nipm}"),
        );

        let manifest = std::fs::read_to_string(done.unwrap().manifest).unwrap();

        assert!(manifest.ends_with(&format!("\n{nipm}")), "{manifest}");
        let unused = events.iter().find_map(|event| match event {
            Event::VipmManifestConverted { unused, .. } => Some(unused.clone()),
            _ => None,
        });
        assert_eq!(unused, Some(vec!["nipm.feeds".to_string()]));
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn a_vipm_toml_the_converter_refuses_writes_nothing_and_says_why_after_its_name() {
        let cases = [
            ("[project", "vipm.toml: "),
            (
                "[dependencies]\na = \">=1.0\"\n",
                "vipm.toml: [dependencies] \">=1.0\": vipm.toml requires an exact version",
            ),
            (
                "[project]\nlabview-version = \"latest\"\n",
                "vipm.toml: [project] labview-version \"latest\" is not a LabVIEW year",
            ),
        ];
        for (text, message) in cases {
            let (done, events, scratch) = init_from_vipm_toml("init-vipm-toml-bad", "App", text);

            let InitError::BadSource { why } = done.unwrap_err() else {
                panic!("{text:?}: not a bad source")
            };

            assert!(why.starts_with(message), "{text:?}: {why}");
            assert!(!why.ends_with('\n'), "{text:?}: {why:?}");
            assert!(events.is_empty(), "{text:?}");
            assert_nothing_written(&scratch.join("App"));
            let _ = std::fs::remove_dir_all(&scratch);
        }
    }

    #[test]
    fn a_vipm_toml_that_is_not_there_is_refused_as_cannot_be_read() {
        let (scratch, root) = folder("init-vipm-toml-missing", "App");

        let why = run(&env(&root), Some(Path::new("vipm.toml")), &no_git).unwrap_err();

        assert!(
            matches!(&why, InitError::BadSource { why } if why.starts_with("vipm.toml: cannot be read: ")),
            "{why:?}"
        );
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn the_from_file_is_read_last_after_project_manifest_and_flag_are_checked() {
        let (scratch, root) = folder("init-from-order", "App");
        let from = Some(Path::new("p.dragon"));
        std::fs::write(root.join("lvpm.toml"), "").unwrap();
        let mut env = env(&root);
        env.flags.labview_version = Some("abc".into());
        env.flags.project = Some(PathBuf::from("Missing"));
        assert!(matches!(
            run(&env, from, &no_git).unwrap_err(),
            InitError::ProjectMissing { .. }
        ));

        env.flags.project = None;
        assert!(matches!(
            run(&env, from, &no_git).unwrap_err(),
            InitError::ManifestExists { .. }
        ));

        std::fs::remove_file(root.join("lvpm.toml")).unwrap();
        assert!(matches!(
            run(&env, from, &no_git).unwrap_err(),
            InitError::BadLabview { .. }
        ));

        env.flags.labview_version = None;
        assert!(matches!(
            run(&env, from, &no_git).unwrap_err(),
            InitError::DragonFile { .. }
        ));
        let _ = std::fs::remove_dir_all(&scratch);
    }
}
