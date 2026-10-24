//! Which operation an `install` request means. One pure function, so that the
//! CLI and any other front end route it the same way.
//!
//! In a project, `install` with no package is a sync of the project to its
//! lock. With a package it adds that package where it was asked to, as it
//! always has.

use super::env::OpEnv;
use crate::errors::{LockIncompatible, RouteError};
use std::path::Path;

/// What an install request means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallMode {
    /// Make the destination equal `lvpm.lock`: bare install, in a project.
    Sync,
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
/// With no package, in a project (with or without a destination flag), it is a
/// sync; a sync takes the lock's full set, so `--no-deps` and `--manifest` are
/// refused. With no package outside a project it is [`InstallMode::Additive`],
/// which fails the way it always did, asking for a package name.
pub fn install_mode(env: &OpEnv, request: &InstallRequest) -> Result<InstallMode, RouteError> {
    if request.package.is_some() {
        return Ok(InstallMode::Additive);
    }
    if env.manifest_for(request.manifest, None)?.is_none() {
        return Ok(InstallMode::Additive);
    }
    let unsupported = match (request.no_deps, request.manifest) {
        (true, _) => Some(LockIncompatible::NoDeps),
        (false, Some(_)) => Some(LockIncompatible::Manifest),
        (false, None) => None,
    };
    match unsupported {
        Some(option) => Err(RouteError::NotWithLock { option }),
        None => Ok(InstallMode::Sync),
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

    const PROJECT: &str = "[project]\nlabview = \"2024\"\n";

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

    #[test]
    fn a_named_package_is_additive_in_a_project_and_outside_one() {
        let request = InstallRequest {
            package: Some("gamma"),
            ..InstallRequest::default()
        };
        for dir in [
            Dir::new("named-in", Some(PROJECT)),
            Dir::new("named-out", None),
        ] {
            assert_eq!(mode(&dir, request).unwrap(), InstallMode::Additive);
        }
    }

    #[test]
    fn a_bare_install_outside_a_project_is_left_to_fail_as_it_always_did() {
        let dir = Dir::new("outside", None);
        assert_eq!(
            mode(&dir, InstallRequest::default()).unwrap(),
            InstallMode::Additive
        );
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
                option: LockIncompatible::NoDeps
            })
        ));

        let manifest = InstallRequest {
            manifest: Some(Path::new(crate::project::FILE_NAME)),
            ..InstallRequest::default()
        };
        assert!(matches!(
            mode(&dir, manifest),
            Err(RouteError::NotWithLock {
                option: LockIncompatible::Manifest
            })
        ));
    }

    #[test]
    fn no_deps_is_fine_for_a_named_package() {
        let dir = Dir::new("named-no-deps", Some(PROJECT));
        let request = InstallRequest {
            package: Some("gamma"),
            no_deps: true,
            ..InstallRequest::default()
        };
        assert_eq!(mode(&dir, request).unwrap(), InstallMode::Additive);
    }
}
