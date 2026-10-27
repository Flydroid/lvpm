//! Which operation an `install` request means. One pure function, so that the
//! CLI and any other front end route it the same way.
//!
//! In a project, `install` with no package is a sync of the project to its
//! lock, and with a package and no destination flag it is an `add`. With a
//! package and a destination flag, or outside a project, it puts that package
//! where it was asked to, as it always has.

use super::env::OpEnv;
use crate::errors::{LockIncompatible, RouteError};
use std::path::Path;

/// What an install request means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallMode {
    /// Make the destination equal `lvpm.lock`: bare install, in a project.
    Sync,
    /// Put the named package in `lvpm.toml`, lock and sync, as `add` does: a
    /// named install in a project, with no destination flag.
    Add,
    /// Put the named package where it was asked to go, and what it needs.
    Additive,
}

/// What an install request asks, before any flag is read as anything else.
#[derive(Debug, Clone, Copy, Default)]
pub struct InstallRequest<'a> {
    pub package: Option<&'a str>,
    /// `--manifest`: the dependencies of another manifest.
    pub manifest: Option<&'a Path>,
    /// `--no-deps`.
    pub no_deps: bool,
}

/// The mode of `request`, in the project `env` runs in, if any.
///
/// A named package is an [`InstallMode::Add`] in a project, unless a
/// destination flag (`--global`, `--prefix`, `--labview-version`) says where it
/// goes, and is additive otherwise. With no package, in a project (with or
/// without a destination flag), it is a sync. Add and sync both take the
/// lock's full set, so `--no-deps` and `--manifest` are refused for them. With
/// no package outside a project there is nothing to install.
pub fn install_mode(env: &OpEnv, request: &InstallRequest) -> Result<InstallMode, RouteError> {
    let mode = match request.package {
        Some(_) => {
            let flags = &env.flags;
            let names_a_destination =
                flags.global || flags.prefix.is_some() || flags.labview_version.is_some();
            if names_a_destination || env.manifest_for(None, None)?.is_none() {
                return Ok(InstallMode::Additive);
            }
            InstallMode::Add
        }
        None => {
            if env.manifest_for(request.manifest, None)?.is_none() {
                return Err(RouteError::NoPackage {
                    searched_from: env.cwd.clone(),
                });
            }
            InstallMode::Sync
        }
    };
    let unsupported = match (request.no_deps, request.manifest) {
        (true, _) => Some(LockIncompatible::NoDeps),
        (false, Some(_)) => Some(LockIncompatible::Manifest),
        (false, None) => None,
    };
    match unsupported {
        Some(option) => Err(RouteError::NotWithLock {
            option,
            adding: mode == InstallMode::Add,
        }),
        None => Ok(mode),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::events::NullSink;
    use std::path::PathBuf;

    /// A directory that is a project when `manifest` says so.
    struct Dir {
        root: PathBuf,
    }

    impl Dir {
        fn new(tag: &str, manifest: Option<&str>) -> Dir {
            let root =
                std::env::temp_dir().join(format!("lvpm-route-test-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            if let Some(text) = manifest {
                std::fs::write(root.join(crate::project::FILE_NAME), text).unwrap();
            }
            Dir { root }
        }

        fn env<'a>(&self, sink: &'a NullSink) -> OpEnv<'a> {
            let config = Config {
                cache: self.root.join("cache"),
                local_sources: Vec::new(),
                venv_dir: PathBuf::from(".lv-venv"),
            };
            OpEnv::new(sink, config, Vec::new(), self.root.clone())
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    const PROJECT: &str = "[project]\nlabview = \">=2024\"\n";

    fn mode(dir: &Dir, request: InstallRequest) -> Result<InstallMode, RouteError> {
        install_mode(&dir.env(&NullSink), &request)
    }

    #[test]
    fn a_bare_install_in_a_project_is_a_sync() {
        let dir = Dir::new("bare", Some(PROJECT));
        assert_eq!(
            mode(&dir, InstallRequest::default()).unwrap(),
            InstallMode::Sync
        );
    }

    #[test]
    fn a_bare_install_is_a_sync_whatever_the_destination_flag() {
        let dir = Dir::new("flags", Some(PROJECT));
        let sink = NullSink;
        for (global, prefix) in [(true, None), (false, Some(PathBuf::from("dest")))] {
            let mut env = dir.env(&sink);
            env.flags.global = global;
            env.flags.prefix = prefix;
            assert_eq!(
                install_mode(&env, &InstallRequest::default()).unwrap(),
                InstallMode::Sync
            );
        }
    }

    const GAMMA: InstallRequest = InstallRequest {
        package: Some("gamma"),
        manifest: None,
        no_deps: false,
    };

    #[test]
    fn a_named_package_in_a_project_is_an_add() {
        let dir = Dir::new("named-in", Some(PROJECT));
        assert_eq!(mode(&dir, GAMMA).unwrap(), InstallMode::Add);
    }

    #[test]
    fn a_named_package_outside_a_project_is_additive() {
        let dir = Dir::new("named-out", None);
        assert_eq!(mode(&dir, GAMMA).unwrap(), InstallMode::Additive);
    }

    #[test]
    fn a_destination_flag_keeps_a_named_package_additive_even_in_a_project() {
        let dir = Dir::new("named-flags", Some(PROJECT));
        let sink = NullSink;
        let flagged: [fn(&mut OpEnv); 3] = [
            |env| env.flags.global = true,
            |env| env.flags.prefix = Some(PathBuf::from("dest")),
            |env| env.flags.labview_version = Some("2026".into()),
        ];
        for set in flagged {
            let mut env = dir.env(&sink);
            set(&mut env);
            assert_eq!(
                install_mode(&env, &GAMMA).unwrap(),
                InstallMode::Additive,
                "{:?}",
                env.flags
            );
        }
    }

    #[test]
    fn a_project_flag_is_no_destination_so_a_named_package_is_still_an_add() {
        let dir = Dir::new("named-project-flag", Some(PROJECT));
        let sink = NullSink;
        let mut env = dir.env(&sink);
        env.flags.project = Some(PathBuf::from("."));
        assert_eq!(install_mode(&env, &GAMMA).unwrap(), InstallMode::Add);
    }

    #[test]
    fn a_bare_install_outside_a_project_has_nothing_to_install() {
        let dir = Dir::new("outside", None);
        let why = mode(&dir, InstallRequest::default()).unwrap_err();
        assert!(
            matches!(&why, RouteError::NoPackage { searched_from } if *searched_from == dir.root),
            "{why:?}"
        );
        assert_eq!(why.to_string(), "give a package name");
    }

    #[test]
    fn no_deps_and_manifest_are_refused_for_a_sync() {
        let dir = Dir::new("refused", Some(PROJECT));
        let no_deps = InstallRequest {
            no_deps: true,
            ..InstallRequest::default()
        };
        assert!(matches!(
            mode(&dir, no_deps),
            Err(RouteError::NotWithLock {
                option: LockIncompatible::NoDeps,
                adding: false
            })
        ));

        let manifest = InstallRequest {
            manifest: Some(Path::new(crate::project::FILE_NAME)),
            ..InstallRequest::default()
        };
        assert!(matches!(
            mode(&dir, manifest),
            Err(RouteError::NotWithLock {
                option: LockIncompatible::Manifest,
                adding: false
            })
        ));
    }

    #[test]
    fn no_deps_is_refused_for_an_add_and_fine_for_a_named_package_that_stays_additive() {
        let request = InstallRequest {
            no_deps: true,
            ..GAMMA
        };
        let in_a_project = Dir::new("named-no-deps", Some(PROJECT));
        assert!(matches!(
            mode(&in_a_project, request),
            Err(RouteError::NotWithLock {
                option: LockIncompatible::NoDeps,
                adding: true
            })
        ));
        let outside = Dir::new("named-no-deps-out", None);
        assert_eq!(mode(&outside, request).unwrap(), InstallMode::Additive);
        let sink = NullSink;
        let mut env = in_a_project.env(&sink);
        env.flags.prefix = Some(PathBuf::from("dest"));
        assert_eq!(install_mode(&env, &request).unwrap(), InstallMode::Additive);
    }
}
