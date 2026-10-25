//! The last step of an install or uninstall: telling a running LabVIEW to
//! rebuild its palettes and menus.

use super::env::OpEnv;
use crate::events::Event;
use crate::refresh;
use crate::target::LvTarget;
use anyhow::{Result, bail};
use std::time::Duration;

/// `lvpm refresh`: rebuild the palettes and menus of the LabVIEW the
/// destination flags select.
pub fn refresh(env: &OpEnv, timeout_secs: u64) -> Result<()> {
    let Some(target) = env.roots()?.target else {
        bail!("refresh needs a real LabVIEW target, not --prefix");
    };
    refresh_palettes_and_menus(env, &target, timeout_secs);
    Ok(())
}

/// Rebuild the palettes and the File/Tools/Help menus from what is now on
/// disk — the last step of an install, after the post-install hooks.
///
/// Without it a package's palette and Tools entries stay invisible until
/// LabVIEW is restarted, however well the files were copied and relinked. See
/// [`refresh`] for why it runs LabVIEW's own VIs rather than the two
/// Application methods directly.
///
/// Best-effort, like the hooks: the files are installed either way, and a
/// refresh that fails costs the user a restart, not the package.
pub fn refresh_palettes_and_menus(env: &OpEnv, target: &LvTarget, timeout_secs: u64) {
    env.sink.emit(Event::RefreshStarted);
    let outcomes = match refresh::run(
        target,
        Duration::from_secs(timeout_secs),
        env.headless,
        env.sink,
    ) {
        Ok(o) => o,
        Err(e) => {
            env.sink.emit(Event::RefreshUnavailable {
                error: format!("{e:#}"),
            });
            return;
        }
    };
    for o in outcomes {
        env.sink.emit(match o.result {
            Ok(took) => Event::RefreshStepDone {
                what: o.what.to_string(),
                took,
            },
            Err(e) => Event::RefreshStepFailed {
                what: o.what.to_string(),
                error: format!("{e:#}"),
            },
        });
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing;
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn a_scratch_prefix_has_no_labview_to_refresh() {
        let mut env = testing::env(Path::new("."));
        env.flags.prefix = Some(PathBuf::from("dest"));
        let why = refresh(&env, 1).unwrap_err().to_string();
        assert_eq!(why, "refresh needs a real LabVIEW target, not --prefix");
    }
}
