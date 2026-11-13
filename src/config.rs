//! lvpm's settings, resolved the way npm resolves its own: `LVPM_CONFIG_<KEY>`
//! in the environment (`lvpm_config_<key>` works too), else the built-in default.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const DEFAULT_VENV_DIR: &str = ".lv-venv";

#[derive(Debug, PartialEq)]
pub struct Config {
    /// Where downloaded indexes (and later, packages) are cached.
    pub cache: PathBuf,
    /// Local directories scanned for package files.
    pub local_sources: Vec<PathBuf>,
    /// Project-relative venv directory, unless configured as an absolute path.
    pub venv_dir: PathBuf,
    /// Where lvpm's logs go: `lvpm.log`, and one file per venv.
    pub log_dir: PathBuf,
}

pub fn load() -> Result<Config> {
    let file = read_file()?;
    resolve(&file, |k| std::env::var(k).ok())
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
struct FileConfig {
    cache: Option<String>,
    sources: SourcesConfig,
    venv: VenvConfig,
    log: LogConfig,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
struct SourcesConfig {
    local: Vec<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
struct VenvConfig {
    dir: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
struct LogConfig {
    dir: Option<String>,
}

fn resolve(file: &FileConfig, env: impl Fn(&str) -> Option<String>) -> Result<Config> {
    let cache = match env_value(&env, "cache") {
        Some(v) => std::path::absolute(v)?,
        None => match &file.cache {
            Some(v) => std::path::absolute(v)?,
            None => default_cache(),
        },
    };
    let local_sources = match env_value(&env, "sources.local") {
        Some(v) => std::env::split_paths(&v).collect(),
        None => file.sources.local.iter().map(PathBuf::from).collect(),
    };
    let venv_dir = env_value(&env, "venv.dir")
        .or_else(|| file.venv.dir.clone().filter(|v| !v.is_empty()))
        .unwrap_or_else(|| DEFAULT_VENV_DIR.to_string());
    let log_dir = match env_value(&env, "log.dir").or_else(|| file.log.dir.clone().filter(|v| !v.is_empty())) {
        Some(v) => std::path::absolute(v)?,
        None => default_log_dir(&cache),
    };
    Ok(Config { cache, local_sources, venv_dir: PathBuf::from(venv_dir), log_dir })
}

/// Where lvpm's logs go, for the logger. Never fails: with a config file that
/// cannot be read the default stands, and the command reports the error
/// itself when it loads the config.
pub fn log_dir() -> PathBuf {
    let env = |k: &str| std::env::var(k).ok();
    read_file()
        .and_then(|file| resolve(&file, env))
        .or_else(|_| resolve(&FileConfig::default(), env))
        .map(|cfg| cfg.log_dir)
        .unwrap_or_else(|_| default_log_dir(&default_cache()))
}

const KEYS: &str = "cache, sources.local, venv.dir, log.dir";

pub fn set(key: &str, values: &[String]) -> Result<()> {
    let mut file = read_file()?;
    match key {
        "cache" if values.len() == 1 => file.cache = Some(values[0].clone()),
        "sources.local" if values.is_empty() => file.sources.local.clear(),
        "sources.local" => {
            file.sources.local = values
                .iter()
                .map(std::path::absolute)
                .collect::<std::io::Result<Vec<_>>>()?
                .into_iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
        }
        "venv.dir" if values.len() == 1 => file.venv.dir = Some(values[0].clone()),
        "log.dir" if values.len() == 1 => {
            file.log.dir = Some(std::path::absolute(&values[0])?.to_string_lossy().into_owned())
        }
        "cache" => bail!("`config set cache` expects one value"),
        "venv.dir" => bail!("`config set venv.dir` expects one value"),
        "log.dir" => bail!("`config set log.dir` expects one value"),
        _ => bail!("unknown config key {key:?}; supported keys: {KEYS}"),
    }
    let path = config_file();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = toml::to_string_pretty(&file)?;
    std::fs::write(&path, text).with_context(|| format!("writing config {}", path.display()))
}

pub fn get(key: &str) -> Result<Vec<String>> {
    let cfg = load()?;
    match key {
        "cache" => Ok(vec![cfg.cache.to_string_lossy().into_owned()]),
        "sources.local" => Ok(cfg.local_sources.iter().map(|p| p.to_string_lossy().into_owned()).collect()),
        "venv.dir" => Ok(vec![cfg.venv_dir.to_string_lossy().into_owned()]),
        "log.dir" => Ok(vec![cfg.log_dir.to_string_lossy().into_owned()]),
        _ => bail!("unknown config key {key:?}; supported keys: {KEYS}"),
    }
}

fn read_file() -> Result<FileConfig> {
    let path = config_file();
    match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text).with_context(|| format!("parsing config {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(FileConfig::default()),
        Err(e) => Err(e).with_context(|| format!("reading config {}", path.display())),
    }
}

fn config_file() -> PathBuf {
    #[cfg(windows)]
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);

    #[cfg(not(windows))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(std::env::temp_dir);

    base.join("lvpm").join("config.toml")
}

/// `LVPM_CONFIG_<KEY>`, else `lvpm_config_<key>`; empty counts as unset.
fn env_value(env: &impl Fn(&str) -> Option<String>, key: &str) -> Option<String> {
    let upper = format!("LVPM_CONFIG_{}", key.replace('.', "_").to_uppercase());
    let lower = format!("lvpm_config_{}", key.replace('.', "_").to_lowercase());
    env(&upper).or_else(|| env(&lower)).filter(|v| !v.is_empty())
}

/// Per-user: `%LOCALAPPDATA%\lvpm\logs` on Windows; on Linux `lvpm/logs` in
/// the cache folder, where lvpm keeps its other state.
fn default_log_dir(cache: &std::path::Path) -> PathBuf {
    #[cfg(windows)]
    {
        let _ = cache;
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("lvpm")
            .join("logs")
    }
    #[cfg(not(windows))]
    {
        cache.join("lvpm").join("logs")
    }
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
        let cfg = resolve(&FileConfig::default(), |_| None).unwrap();
        assert_eq!(cfg.cache, default_cache());
        assert!(cfg.local_sources.is_empty());
        assert_eq!(cfg.venv_dir, PathBuf::from(DEFAULT_VENV_DIR));
        assert_eq!(cfg.log_dir, default_log_dir(&cfg.cache));
    }

    #[test]
    fn env_overrides_default_in_either_case_and_empty_is_unset() {
        let upper = |k: &str| (k == "LVPM_CONFIG_CACHE").then(|| "/from/env".to_string());
        assert_eq!(
            resolve(&FileConfig::default(), upper).unwrap().cache,
            std::path::absolute("/from/env").unwrap()
        );

        let lower = |k: &str| (k == "lvpm_config_cache").then(|| "/lower".to_string());
        assert_eq!(
            resolve(&FileConfig::default(), lower).unwrap().cache,
            std::path::absolute("/lower").unwrap()
        );

        let empty = |k: &str| (k == "LVPM_CONFIG_CACHE").then(String::new);
        assert_eq!(resolve(&FileConfig::default(), empty).unwrap().cache, default_cache());
    }

    #[test]
    fn file_settings_are_overridden_by_environment_values() {
        let file = FileConfig {
            cache: Some("from-file".into()),
            sources: SourcesConfig { local: vec!["packages-a".into()] },
            venv: VenvConfig { dir: Some(".from-file".into()) },
            log: LogConfig { dir: Some("logs-from-file".into()) },
        };
        let from_file = resolve(&file, |_| None).unwrap();
        assert_eq!(from_file.venv_dir, PathBuf::from(".from-file"));
        assert_eq!(from_file.log_dir, std::path::absolute("logs-from-file").unwrap());
        let cfg = resolve(&file, |key| match key {
            "LVPM_CONFIG_CACHE" => Some("from-env".into()),
            "LVPM_CONFIG_SOURCES_LOCAL" => Some("packages-b".into()),
            "LVPM_CONFIG_VENV_DIR" => Some(".custom-venv".into()),
            "LVPM_CONFIG_LOG_DIR" => Some("logs-from-env".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(cfg.log_dir, std::path::absolute("logs-from-env").unwrap());
        assert_eq!(cfg.cache, std::path::absolute("from-env").unwrap());
        assert_eq!(cfg.local_sources, std::env::split_paths("packages-b").collect::<Vec<_>>());
        assert_eq!(cfg.venv_dir, PathBuf::from(".custom-venv"));
    }

    #[test]
    fn config_serializes_local_sources_as_a_list() {
        let file = FileConfig {
            cache: None,
            sources: SourcesConfig { local: vec!["one".into(), "two".into()] },
            venv: VenvConfig::default(),
            log: LogConfig::default(),
        };
        let encoded = toml::to_string_pretty(&file).unwrap();
        let decoded: FileConfig = toml::from_str(&encoded).unwrap();
        assert_eq!(decoded.sources.local, ["one", "two"]);
    }
}
