//! Questions about what lvpm knows, answered as data: the front end decides
//! how many rows to show and what to call them.

use super::env::OpEnv;
use crate::config;
use crate::index::{self, Entry, Import};
use crate::install::{self, Manifest};
use crate::paths;
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
/// is what the command started with and is not updated. A relative path is
/// taken from the directory the command was run from.
pub fn config_set(env: &OpEnv, key: &str, values: &[String]) -> Result<()> {
    config::set(key, values, &env.cwd)
}

/// Import a package archive, or the packages directly inside a directory, into
/// the shared cache. A relative `path` is taken from the directory the command
/// was run from.
pub fn cache_add(env: &OpEnv, path: &Path) -> Result<Vec<Import>> {
    index::cache_add(
        &env.config.cache,
        &paths::resolve(&env.cwd, path),
        env.refresh,
    )
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
    fn cache_add_takes_a_relative_path_from_the_environments_directory_not_the_processs() {
        use std::io::Write;

        let cwd = super::super::testing::scratch_dir("cache-add-cwd");
        std::fs::create_dir_all(cwd.join("pkgs")).unwrap();
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        writer
            .start_file("spec", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer
            .write_all(b"[Package]\nName=alpha\nVersion=1.0\n")
            .unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        std::fs::write(cwd.join("pkgs").join("alpha-1.0.vip"), bytes).unwrap();

        let config = Config {
            cache: cwd.join("cache"),
            local_sources: Vec::new(),
            venv_dir: PathBuf::from(".lv-venv"),
        };
        let env = OpEnv::new(&NullSink, config, Vec::new(), cwd.clone());
        let imports = cache_add(&env, Path::new("pkgs")).unwrap();
        assert_eq!(imports.len(), 1);
        assert_eq!(imports[0].path, cwd.join("pkgs").join("alpha-1.0.vip"));
        let _ = std::fs::remove_dir_all(&cwd);
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
