//! `lvpm launch`: start LabVIEW on the project's venv.
//!
//! No relinking here — that happened at install — and no waiting: the IDE is
//! the user's from the moment it appears.

use super::env::{Destination, OpEnv};
use crate::launch;
use crate::paths;
use crate::venv::Venv;
use anyhow::{Result, bail, ensure};
use std::path::{Path, PathBuf};

/// A LabVIEW started on a venv.
#[derive(Debug, Clone)]
pub struct Launched {
    pub venv: Venv,
    /// The ini it was started with.
    pub ini: PathBuf,
    /// The project file it was asked to open.
    pub lvproj: Option<PathBuf>,
}

/// Start the venv's LabVIEW, optionally opening `lvproj` in it.
pub fn launch(env: &OpEnv, lvproj: Option<&Path>) -> Result<Launched> {
    let Destination::Venv(venv) = env.destination()? else {
        bail!("launch needs a venv: run it inside a project that has one, or pass --project <DIR>");
    };
    let venv = crate::venv::ensure_port(&venv, env.sink)?;
    if launch::is_running(&venv) {
        bail!(
            "a LabVIEW is already running or starting on this venv (VI Server port {}) — open the \
             project from that window, or close it and launch again",
            venv.port
        );
    }
    let lvproj = match lvproj {
        Some(p) => {
            let p = paths::resolve(&env.cwd, p);
            ensure!(p.is_file(), "no such project file: {}", p.display());
            Some(p)
        }
        None => None,
    };
    let ini = launch::write_ini(&venv)?;
    launch::spawn(&venv, &ini, lvproj.as_deref(), env.sink)?;
    Ok(Launched { venv, ini, lvproj })
}

#[cfg(test)]
mod tests {
    use super::super::testing::{self, scratch_dir};
    use super::*;

    #[test]
    fn launching_needs_a_venv() {
        let empty = scratch_dir("launch-none");
        let mut env = testing::env(&empty);
        env.flags.labview_version = Some("2026".into());
        let why = launch(&env, None).unwrap_err().to_string();
        assert!(why.starts_with("launch needs a venv:"), "{why}");
        let _ = std::fs::remove_dir_all(&empty);
    }

    #[test]
    fn a_project_file_that_is_not_there_stops_the_launch_before_anything_starts() {
        let repo = scratch_dir("launch-lvproj");
        let venv = testing::venv_in(&repo);
        let env = testing::env(&repo);

        let why = launch(&env, Some(Path::new("missing.lvproj")))
            .unwrap_err()
            .to_string();
        assert_eq!(
            why,
            format!(
                "no such project file: {}",
                repo.join("missing.lvproj").display()
            )
        );
        assert!(
            !venv.ini_path().exists(),
            "the ini is written only for a launch"
        );

        let _ = std::fs::remove_dir_all(&repo);
    }
}
