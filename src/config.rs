//! lvpm's settings, resolved the way npm resolves its own: `LVPM_CONFIG_<KEY>`
//! in the environment (`lvpm_config_<key>` works too), else the built-in default.

use anyhow::Result;
use std::path::PathBuf;

#[derive(Debug, PartialEq)]
pub struct Config {
    /// Where downloaded indexes (and later, packages) are cached.
    pub cache: PathBuf,
}

pub fn load() -> Result<Config> {
    resolve(|k| std::env::var(k).ok())
}

fn resolve(env: impl Fn(&str) -> Option<String>) -> Result<Config> {
    let cache = match env_value(&env, "cache") {
        Some(v) => std::path::absolute(v)?,
        None => default_cache(),
    };
    Ok(Config { cache })
}

/// `LVPM_CONFIG_<KEY>`, else `lvpm_config_<key>`; empty counts as unset.
fn env_value(env: &impl Fn(&str) -> Option<String>, key: &str) -> Option<String> {
    let upper = format!("LVPM_CONFIG_{}", key.to_uppercase());
    let lower = format!("lvpm_config_{}", key.to_lowercase());
    env(&upper).or_else(|| env(&lower)).filter(|v| !v.is_empty())
}

/// Per-user, not per-target — the feeds are the same for every LabVIEW.
#[cfg(windows)]
fn default_cache() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("lvpm")
        .join("cache")
}

#[cfg(not(windows))]
fn default_cache() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("lvpm").join("cache")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_when_nothing_is_set() {
        assert_eq!(resolve(|_| None).unwrap().cache, default_cache());
    }

    #[test]
    fn env_overrides_default_in_either_case_and_empty_is_unset() {
        let upper = |k: &str| (k == "LVPM_CONFIG_CACHE").then(|| "/from/env".to_string());
        assert_eq!(resolve(upper).unwrap().cache, std::path::absolute("/from/env").unwrap());

        let lower = |k: &str| (k == "lvpm_config_cache").then(|| "/lower".to_string());
        assert_eq!(resolve(lower).unwrap().cache, std::path::absolute("/lower").unwrap());

        let empty = |k: &str| (k == "LVPM_CONFIG_CACHE").then(String::new);
        assert_eq!(resolve(empty).unwrap().cache, default_cache());
    }
}
