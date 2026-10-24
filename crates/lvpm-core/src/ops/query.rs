//! Questions about what lvpm knows, answered as data: the front end decides
//! how many rows to show and what to call them.

use super::env::OpEnv;
use crate::config;
use crate::index::{self, Entry, Import};
use crate::install::{self, Manifest};
use crate::target::LvTarget;
use anyhow::Result;
use std::path::Path;

/// The LabVIEW installations detected on this machine, newest first.
pub fn targets<'e>(env: &'e OpEnv) -> &'e [LvTarget] {
    &env.targets
}

/// Every package whose name or display name contains `query`, newest version
/// of each, by name. All of them: a front end that shows fewer says so itself.
pub fn search(env: &OpEnv, query: &str) -> Result<Vec<Entry>> {
    let manifest = env.manifest_for(None, None)?;
    let index = env.load_index(manifest.as_ref())?;
    Ok(index.search(query).into_iter().cloned().collect())
}

/// What lvpm has installed into the selected destination.
pub fn list(env: &OpEnv) -> Result<Vec<Manifest>> {
    install::list_installed(&env.destination()?.store())
}

/// The effective value of a setting, one string per value.
pub fn config_get(env: &OpEnv, key: &str) -> Result<Vec<String>> {
    config::get(&env.config, key)
}

/// Change a user-level setting. Writes the user's config file; `env.config`
/// is what the command started with and is not updated.
pub fn config_set(key: &str, values: &[String]) -> Result<()> {
    config::set(key, values)
}

/// Import a package archive, or the packages directly inside a directory, into
/// the shared cache.
pub fn cache_add(env: &OpEnv, path: &Path) -> Result<Vec<Import>> {
    index::cache_add(&env.config.cache, path, env.refresh)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::events::NullSink;
    use std::path::PathBuf;

    fn env_with(cache: &str, venv_dir: &str, targets: Vec<LvTarget>) -> OpEnv<'static> {
        let config = Config {
            cache: PathBuf::from(cache),
            local_sources: vec![PathBuf::from("packages-a"), PathBuf::from("packages-b")],
            venv_dir: PathBuf::from(venv_dir),
        };
        OpEnv::new(&NullSink, config, targets, PathBuf::from("."))
    }

    #[test]
    fn the_detected_installations_are_the_ones_the_environment_was_given() {
        let labview = LvTarget {
            version: 26.3,
            bitness: 64,
            path: PathBuf::from("LabVIEW 2026"),
        };
        let env = env_with("cache", ".lv-venv", vec![labview.clone()]);
        assert_eq!(targets(&env).len(), 1);
        assert_eq!(targets(&env)[0].path, labview.path);
        assert!(targets(&env_with("cache", ".lv-venv", Vec::new())).is_empty());
    }

    #[test]
    fn a_setting_is_read_from_the_environments_config_not_the_machines() {
        let env = env_with("the-cache", ".the-venv", Vec::new());
        assert_eq!(config_get(&env, "venv.dir").unwrap(), [".the-venv"]);
        assert_eq!(
            config_get(&env, "sources.local").unwrap(),
            ["packages-a", "packages-b"]
        );
        let cache = config_get(&env, "cache").unwrap();
        assert_eq!(cache, [PathBuf::from("the-cache").to_string_lossy()]);
        let why = config_get(&env, "nothing").unwrap_err().to_string();
        assert!(why.starts_with("unknown config key \"nothing\""), "{why}");
    }
}
