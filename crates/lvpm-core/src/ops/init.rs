//! `lvpm init`: a folder becomes a project.
//!
//! All checks come first and write nothing; then `lvpm.toml` is written once,
//! with `create_new`, so a manifest that appears after the check is refused too.

use super::env::OpEnv;
use crate::errors::InitError;
use crate::events::{Event, Retarget};
use crate::target::LvTarget;
use crate::{manifest_edit, paths, project};
use anyhow::anyhow;
use serde::Serialize;
use std::cmp::Reverse;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// The version a new project starts at.
const FIRST_VERSION: &str = "0.1.0";

/// What `init` made.
#[derive(Debug, Clone, Serialize)]
pub struct Initialized {
    /// The `lvpm.toml` it wrote.
    pub manifest: PathBuf,
    /// The year its `labview` states, if it has one.
    pub labview: Option<u32>,
}

/// The LabVIEW the manifest names, and how to choose another installation.
#[derive(Debug, PartialEq)]
struct Choice {
    year: Option<u32>,
    /// Every installation, newest first.
    installed: Vec<String>,
    retarget: Option<Retarget>,
}

/// `--labview-version`'s `flag` year, else the newest installation's. Of two
/// bitnesses of one year the 64-bit one wins, whatever order `targets` is in.
///
/// `retarget` is the next installation after the chosen one, and there is none
/// when no installation was chosen. A flag year that is not installed is still
/// the year written.
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
    let retarget = chosen
        .zip(other)
        .map(|(chosen, other)| match other.year() == chosen.year() {
            true => Retarget::Path(other.path.clone()),
            false => Retarget::Year(other.year()),
        });
    Choice {
        year: flag.or(chosen.map(LvTarget::year)),
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
pub fn run(env: &OpEnv) -> Result<Initialized, InitError> {
    let root = match &env.flags.project {
        Some(dir) => paths::resolve(&env.cwd, dir),
        None => paths::clean(&env.cwd),
    };
    if !root.exists() {
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
            project::labview_year(value).map_err(|why| InitError::BadLabview {
                value: value.to_string(),
                why,
            })
        })
        .transpose()?;
    let choice = choose_labview(&env.targets, flag);

    let text = manifest_edit::skeleton(
        &manifest_edit::folder_name(&root),
        Some(FIRST_VERSION),
        choice.year,
    );
    write_new(&manifest, &text)?;
    env.sink.emit(Event::InitManifestWritten {
        path: manifest.clone(),
        labview: choice.year,
        installed: choice.installed,
        retarget: choice.retarget,
    });
    if choice.year.is_none() {
        env.sink.emit(Event::InitNoLabview);
    }
    Ok(Initialized {
        manifest,
        labview: choice.year,
    })
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
            let done = run(&env).unwrap();
            Ran {
                events: sink.events(),
                manifest: std::fs::read_to_string(&done.manifest).unwrap(),
                done,
                scratch,
            }
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

        let done = run(&env).unwrap();

        assert_eq!(done.manifest, root.join("lvpm.toml"));
        assert_eq!(done.labview, Some(2023));
        assert_eq!(
            std::fs::read_to_string(&done.manifest).unwrap(),
            "[project]\nname = \"Serializer\"\nversion = \"0.1.0\"\nlabview = \">=2023\"\n\n[dependencies]\n"
        );
        assert_eq!(
            sink.events(),
            [Event::InitManifestWritten {
                path: done.manifest.clone(),
                labview: Some(2023),
                installed: vec!["2026 (64-bit)".into()],
                retarget: None,
            }]
        );
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
    fn a_flag_year_that_is_not_installed_is_written_and_offers_nothing() {
        let ran = Ran::init(
            "init-flag-absent",
            vec![lv(2026, 64), lv(2024, 64)],
            Some("2025"),
        );

        assert_eq!(ran.written().0, Some(2025));
        assert!(
            ran.manifest.contains("labview = \">=2025\""),
            "{}",
            ran.manifest
        );
        assert_eq!(ran.written().2, None);
        assert_eq!(ran.events.len(), 1, "no warning when the flag named one");
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
        assert_eq!(ran.events[1..], [Event::InitNoLabview]);
    }

    #[test]
    fn with_no_labview_a_flag_still_names_the_year_and_nothing_warns() {
        let ran = Ran::init("init-no-labview-flag", Vec::new(), Some("2023"));

        assert!(
            ran.manifest.contains("labview = \">=2023\""),
            "{}",
            ran.manifest
        );
        assert_eq!(ran.events.len(), 1, "{:?}", ran.events);
    }

    #[test]
    fn project_names_the_folder_and_is_taken_from_the_working_directory() {
        let (scratch, root) = folder("init-project", "Elsewhere");
        let mut env = env(&scratch);
        env.flags.project = Some(PathBuf::from("Elsewhere"));

        let done = run(&env).unwrap();

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

        let why = run(&env).unwrap_err();

        assert!(
            matches!(&why, InitError::ProjectMissing { dir } if *dir == scratch.join("Missing")),
            "{why:?}"
        );
        assert!(!scratch.join("Missing").exists());
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn an_existing_manifest_is_refused_untouched_with_nothing_said() {
        let (scratch, root) = folder("init-exists", "Taken");
        std::fs::write(root.join("lvpm.toml"), "# mine\n").unwrap();
        let sink = Recorded::default();

        let why = run(&env_on(&sink, &root)).unwrap_err();

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
    fn a_manifest_that_appears_after_the_check_is_not_overwritten() {
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

        let done = run(&env_on(&Recorded::default(), &app)).unwrap();

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

            let why = run(&env).unwrap_err();

            assert!(
                matches!(&why, InitError::BadLabview { value: v, .. } if v == value),
                "{value:?}: {why:?}"
            );
            assert!(!root.join("lvpm.toml").exists(), "{value:?}");
            let _ = std::fs::remove_dir_all(&scratch);
        }
    }

    #[test]
    fn the_checks_run_in_order_the_folder_then_the_manifest_then_the_flag() {
        let (scratch, root) = folder("init-order", "Taken");
        std::fs::write(root.join("lvpm.toml"), "").unwrap();
        let mut env = env(&root);
        env.flags.labview_version = Some("abc".into());
        assert!(matches!(
            run(&env).unwrap_err(),
            InitError::ManifestExists { .. }
        ));

        env.flags.project = Some(PathBuf::from("Missing"));
        assert!(matches!(
            run(&env).unwrap_err(),
            InitError::ProjectMissing { .. }
        ));
        let _ = std::fs::remove_dir_all(&scratch);
    }
}
