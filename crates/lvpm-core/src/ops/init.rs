//! `lvpm init`: a folder becomes a project.
//!
//! All checks come first and write nothing; then `lvpm.toml` is written once,
//! with `create_new`, so a manifest that appears after the check is refused too.

use super::env::OpEnv;
use crate::errors::InitError;
use crate::events::Event;
use crate::{manifest_edit, paths, project};
use anyhow::anyhow;
use serde::Serialize;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// The version a new project starts at.
const FIRST_VERSION: &str = "0.1.0";

/// What `init` made.
#[derive(Debug, Clone, Serialize)]
pub struct Initialized {
    /// The `lvpm.toml` it wrote.
    pub manifest: PathBuf,
}

/// Write an `lvpm.toml` in the project root — `--project`, else the directory
/// the command ran in — named after its folder. `labview` is `--labview-version`
/// when given.
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
    let labview = env
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

    let text = manifest_edit::skeleton(
        &manifest_edit::folder_name(&root),
        Some(FIRST_VERSION),
        labview,
    );
    write_new(&manifest, &text)?;
    env.sink.emit(Event::InitManifestWritten {
        path: manifest.clone(),
        labview,
    });
    Ok(Initialized { manifest })
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

    #[test]
    fn init_writes_the_skeleton_named_after_its_folder_and_says_so() {
        let (scratch, root) = folder("init-writes", "Serializer");
        let sink = Recorded::default();
        let mut env = env_on(&sink, &root);
        env.flags.labview_version = Some("2023".into());

        let done = run(&env).unwrap();

        assert_eq!(done.manifest, root.join("lvpm.toml"));
        assert_eq!(
            std::fs::read_to_string(&done.manifest).unwrap(),
            "[project]\nname = \"Serializer\"\nversion = \"0.1.0\"\nlabview = \">=2023\"\n\n[dependencies]\n"
        );
        assert_eq!(
            sink.events(),
            [Event::InitManifestWritten {
                path: done.manifest.clone(),
                labview: Some(2023)
            }]
        );
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn without_a_labview_version_the_manifest_has_no_labview() {
        let (scratch, root) = folder("init-no-labview", "App");

        let done = run(&env_on(&Recorded::default(), &root)).unwrap();

        let text = std::fs::read_to_string(done.manifest).unwrap();
        assert_eq!(
            text,
            "[project]\nname = \"App\"\nversion = \"0.1.0\"\n\n[dependencies]\n"
        );
        let _ = std::fs::remove_dir_all(&scratch);
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
