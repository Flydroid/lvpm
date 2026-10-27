//! Operations: what a front end asks core to do. Each takes an [`OpEnv`] and
//! reports through its sink; none prints, reads the environment or knows which
//! front end called it.

pub mod add_remove;
pub mod build;
pub mod env;
pub mod hooks;
pub mod install;
pub mod launch_cmd;
pub mod lock;
pub mod publish;
pub mod query;
pub mod refresh;
pub mod relink;
pub mod route;
pub mod session;
pub mod snapshot;
pub mod start;
pub mod sync;
#[cfg(test)]
pub(crate) mod testing;
pub mod uninstall;
pub mod venv_cmd;

pub use env::{Destination, DestinationFlags, OpEnv};

use anyhow::Context;

/// The project's root and its validated `[distribution]`, for `lvpm <command>`.
pub(super) fn packaged(
    env: &OpEnv,
    command: &str,
) -> anyhow::Result<(std::path::PathBuf, crate::distribution::Distribution)> {
    let Some((manifest, p)) = env.manifest_for(None, None)? else {
        anyhow::bail!(
            "no {} at or above {}: lvpm {command} works on a project",
            crate::project::FILE_NAME,
            env.cwd.display()
        );
    };
    let root = manifest.parent().context("the manifest has no folder")?;
    let d = crate::distribution::validate(&p, root)?;
    Ok((root.to_path_buf(), d))
}

/// Where `lvpm build` puts, and `lvpm publish` finds, the current version's package.
pub(super) fn package_path(
    root: &std::path::Path,
    d: &crate::distribution::Distribution,
) -> std::path::PathBuf {
    root.join("dist").join(d.file_name())
}
