//! `lvpm venv create | status | remove`.
//!
//! Removing asks nothing here: the front end finds the venv with [`existing`],
//! asks the user, and says what the answer was to [`remove`] (AC-K6).

use super::env::OpEnv;
use crate::venv::{self, Venv};
use crate::{install, launch};
use anyhow::{Result, bail};

/// Create the venv of the project the command is run in, or of `--project`,
/// and bind it to a LabVIEW: `--labview-version`, else the manifest's.
pub fn create(env: &OpEnv) -> Result<Venv> {
    let start = match &env.flags.project {
        Some(project) => env.cwd.join(project),
        None => env.cwd.clone(),
    };
    venv::create(
        &start,
        env.flags.labview_version.as_deref(),
        &env.config.venv_dir,
        &env.targets,
        env.sink,
    )
}

/// What `lvpm venv status` reports about the venv a command run here would use.
#[derive(Debug, Clone)]
pub struct VenvStatus {
    pub venv: Venv,
    /// Packages installed in the venv.
    pub installed: usize,
    /// Of those, the ones the relink pass has not run over yet.
    pub not_relinked: usize,
    /// A LabVIEW is answering on the venv's VI Server port.
    pub running: bool,
}

/// The venv a command run here would use, if there is one.
pub fn status(env: &OpEnv) -> Result<Option<VenvStatus>> {
    let Some(venv) = env.find_venv()? else {
        return Ok(None);
    };
    let installed = install::list_installed(&venv.store())?;
    Ok(Some(VenvStatus {
        installed: installed.len(),
        not_relinked: installed.iter().filter(|m| !m.relinked).count(),
        running: launch::is_listening(venv.port),
        venv,
    }))
}

/// The venv a removal means. There must be one.
pub fn existing(env: &OpEnv) -> Result<Venv> {
    match env.find_venv()? {
        Some(venv) => Ok(venv),
        None => bail!("no venv at or above {}", env.cwd.display()),
    }
}

/// Delete the venv and everything installed in it, if `confirmed`; a venv that
/// was not confirmed is left as it is.
pub fn remove(env: &OpEnv, venv: &Venv, confirmed: bool) -> Result<()> {
    venv::remove(venv, confirmed, env.sink)
}

#[cfg(test)]
mod tests {
    use super::super::testing::{self, scratch_dir};
    use super::*;

    #[test]
    fn status_finds_the_venv_from_below_and_counts_what_is_in_it() {
        let repo = scratch_dir("venv-status");
        let created = testing::venv_in(&repo);
        let deep = repo.join("src");
        std::fs::create_dir_all(&deep).unwrap();

        let found = status(&testing::env(&deep)).unwrap().unwrap();
        assert_eq!(found.venv.dir, created.dir);
        assert_eq!((found.installed, found.not_relinked), (0, 0));
        assert!(!found.running);

        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn with_no_venv_there_is_none_to_report_and_none_to_remove() {
        let empty = scratch_dir("venv-none");
        let env = testing::env(&empty);
        assert!(status(&env).unwrap().is_none());
        let why = existing(&env).unwrap_err().to_string();
        assert_eq!(why, format!("no venv at or above {}", empty.display()));
        let _ = std::fs::remove_dir_all(&empty);
    }

    #[test]
    fn a_venv_is_deleted_only_when_the_caller_says_it_was_confirmed() {
        let repo = scratch_dir("venv-remove");
        let created = testing::venv_in(&repo);
        let env = testing::env(&repo);

        let why = remove(&env, &created, false).unwrap_err().to_string();
        assert_eq!(why, "not removed");
        assert!(created.dir.is_dir());

        remove(&env, &created, true).unwrap();
        assert!(!created.dir.exists());

        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn create_binds_the_project_the_flags_name() {
        let repo = scratch_dir("venv-create");
        let elsewhere = scratch_dir("venv-create-cwd");
        let mut env = testing::env(&elsewhere);
        env.flags.project = Some(repo.clone());
        env.flags.labview_version = Some("2026".into());

        let created = create(&env).unwrap();
        assert_eq!(
            created.dir,
            std::path::absolute(&repo).unwrap().join(".lv-venv")
        );
        assert!(!elsewhere.join(".lv-venv").exists());

        let _ = std::fs::remove_dir_all(&repo);
        let _ = std::fs::remove_dir_all(&elsewhere);
    }
}
