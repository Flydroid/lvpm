//! VIPM interoperability: VIPM's project files read into lvpm's model.
//!
//! `vipm.toml` (VIPM 2026 and later) is converted field by field into a
//! [`Project`], so the rest of lvpm only ever sees one manifest model. The
//! mapping follows VIPM's published documentation (docs.vipm.io) and is listed
//! in docs/manifest.md. Every conversion is announced on stderr, naming the
//! fields with no lvpm equivalent rather than dropping them silently.

use crate::project::{Constraint, Group, Project};
use crate::version::Version;
use anyhow::{Context, Result, bail};
use std::path::Path;

/// VIPM's manifest file name.
pub const TOML_FILE_NAME: &str = "vipm.toml";

/// The lvpm dependency group VIPM's `[dev-dependencies]` becomes.
pub const DEV_GROUP: &str = "dev";

/// A converted `vipm.toml`.
#[derive(Debug)]
pub struct Converted {
    pub project: Project,
    /// Dotted names of the fields lvpm did not use, in file order.
    pub unused: Vec<String>,
}

/// Whether `path` names a `vipm.toml` (case-insensitive, as on Windows).
pub fn is_toml(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(TOML_FILE_NAME))
}

/// Read and convert a `vipm.toml`, reporting the conversion on stderr.
pub fn read_toml(path: &Path) -> Result<Project> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading manifest {}", path.display()))?;
    let c = parse_toml(&text).with_context(|| format!("parsing VIPM manifest {}", path.display()))?;
    eprintln!("manifest: {} (VIPM format, converted)", path.display());
    if !c.unused.is_empty() {
        eprintln!("  not used: {}", c.unused.join(", "));
    }
    if let Some(lv) = &c.project.labview {
        eprintln!("  labview-version {lv:?} read as minimum labview = {lv:?}");
    }
    if c.project.groups.iter().any(|(n, _)| n == DEV_GROUP) {
        eprintln!("  [dev-dependencies] read as group {DEV_GROUP:?} (install with --with {DEV_GROUP})");
    }
    Ok(c.project)
}

pub fn parse_toml(text: &str) -> Result<Converted> {
    let doc: toml::Table = toml::from_str(text)?;
    let mut project = Project::default();
    let mut unused = Vec::new();
    for (key, value) in doc {
        match key.as_str() {
            "project" => {
                for (k, v) in table(&key, value)? {
                    match k.as_str() {
                        "name" => project.name = Some(string(&key, &k, v)?),
                        "version" => project.version = Some(string(&key, &k, v)?),
                        // VIPM pins the year; lvpm reads it as the minimum.
                        "labview-version" => project.labview = Some(string(&key, &k, v)?),
                        _ => unused.push(format!("{key}.{k}")),
                    }
                }
            }
            "dependencies" => {
                for (name, v) in table(&key, value)? {
                    let s = string(&key, &name, v)?;
                    project.dependencies.push((name, exact(&key, &s)?));
                }
            }
            "dev-dependencies" => {
                let mut dev = Group::default();
                for (name, v) in table(&key, value)? {
                    let s = string(&key, &name, v)?;
                    dev.dependencies.push((name, exact(&key, &s)?));
                }
                project.groups.push((DEV_GROUP.to_string(), dev));
            }
            "nipm" => {
                for (k, v) in table(&key, value)? {
                    match k.as_str() {
                        "dependencies" => project.nipm = table(&k, v)?.into_iter().map(|(n, _)| n).collect(),
                        _ => unused.push(format!("{key}.{k}")),
                    }
                }
            }
            "build" => unused.extend(table(&key, value)?.keys().map(|k| format!("{key}.{k}"))),
            _ => unused.push(key),
        }
    }
    Ok(Converted { project, unused })
}

fn table(key: &str, value: toml::Value) -> Result<toml::Table> {
    match value {
        toml::Value::Table(t) => Ok(t),
        other => bail!("{key}: expected a table, got {}", other.type_str()),
    }
}

fn string(table: &str, key: &str, value: toml::Value) -> Result<String> {
    match value {
        toml::Value::String(s) => Ok(s),
        other => bail!("[{table}] {key}: expected a string, got {}", other.type_str()),
    }
}

/// VIPM writes exact versions only and rejects empty ones.
fn exact(table: &str, version: &str) -> Result<Constraint> {
    let v = version.trim();
    if v.is_empty() || v.starts_with(['<', '>', '=', '~', '^', '*']) {
        bail!("[{table}] {version:?}: vipm.toml requires an exact version, e.g. \"1.2.3.4\"");
    }
    Ok(Constraint::Exact(Version::parse(v)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_the_documented_complete_example() {
        let c = parse_toml(
            r#"
[project]
name = "instrument-control"
version = "1.0.0"
labview-version = "2024"
labview-bitness = 64
description = "Automated instrument control"

[dependencies]
oglib_array = "6.0.1.20"
jki_lib_state_machine = "2.0.0.50"

[dev-dependencies]
caraya = "1.4.5.165"

[nipm.feeds]
ni-daq = "https://download.ni.com/support/nipkg/products/ni-d/ni-daqmx/24.5/released/"

[nipm.dependencies]
ni-daqmx = { version = "24.5.0", feed = "ni-daq" }
ni-visa = "24.0"

[nipm.dev-dependencies]
ni-teststand = "2023.1.0"

[build.main_library]
type = "ppl"
top-level-library = "src/InstrumentControl.lvlib"

[build.package]
type = "vipb"
path = "InstrumentControl.vipb"
"#,
        )
        .unwrap();
        let p = &c.project;
        assert_eq!(p.name.as_deref(), Some("instrument-control"));
        assert_eq!(p.version.as_deref(), Some("1.0.0"));
        assert_eq!(p.labview.as_deref(), Some("2024"));
        assert_eq!(
            p.dependencies,
            vec![
                ("oglib_array".into(), Constraint::Exact(Version::parse("6.0.1.20"))),
                ("jki_lib_state_machine".into(), Constraint::Exact(Version::parse("2.0.0.50"))),
            ]
        );
        assert_eq!(p.nipm, ["ni-daqmx", "ni-visa"]);
        assert_eq!(
            p.groups,
            vec![(
                "dev".to_string(),
                Group {
                    include: vec![],
                    dependencies: vec![("caraya".into(), Constraint::Exact(Version::parse("1.4.5.165")))],
                }
            )]
        );
        assert!(p.sources.is_empty());
        assert!(p.default_sources);
        assert_eq!(
            c.unused,
            [
                "project.labview-bitness",
                "project.description",
                "nipm.feeds",
                "nipm.dev-dependencies",
                "build.main_library",
                "build.package",
            ]
        );
    }

    #[test]
    fn dev_dependencies_are_selected_like_any_lvpm_group() {
        let c = parse_toml("[dependencies]\na = \"1.0\"\n[dev-dependencies]\nb = \"2.0\"\n").unwrap();
        let none: Vec<String> = Vec::new();
        let names = |s: crate::project::Selection| -> Vec<String> {
            s.dependencies.into_iter().map(|(n, _)| n).collect()
        };
        assert_eq!(names(c.project.select(&none, &none, &none).unwrap()), ["a"]);
        assert_eq!(names(c.project.select(&["dev".into()], &none, &none).unwrap()), ["a", "b"]);
        assert!(parse_toml("[dev-dependencies]\nb = \">=2.0\"\n").is_err(), "exact only");
    }

    #[test]
    fn only_exact_versions_are_accepted() {
        for bad in ["\"\"", "\"  \"", "\">=1.0\"", "\"*\"", "\"=1.0\"", "\"^1.0\""] {
            let text = format!("[dependencies]\noglib_array = {bad}\n");
            assert!(parse_toml(&text).is_err(), "{bad} should be rejected");
        }
        assert!(parse_toml("[dependencies]\noglib_array = 6.0\n").is_err(), "unquoted");
    }

    #[test]
    fn wrong_shapes_are_reported_by_key() {
        let err = parse_toml("project = \"x\"\n").unwrap_err().to_string();
        assert!(err.contains("project"), "{err}");
        let err = parse_toml("[project]\nlabview-version = 2024\n").unwrap_err().to_string();
        assert!(err.contains("labview-version"), "{err}");
    }

    #[test]
    fn recognises_vipm_toml_by_file_name_only() {
        assert!(is_toml(Path::new("C:/proj/vipm.toml")));
        assert!(is_toml(Path::new("VIPM.TOML")));
        assert!(!is_toml(Path::new("C:/proj/lvpm.toml")));
        assert!(!is_toml(Path::new("C:/vipm.toml/lvpm.toml")));
    }

    #[test]
    fn a_minimal_file_converts_to_an_empty_project() {
        let c = parse_toml("[project]\nname = \"x\"\n").unwrap();
        assert!(c.project.dependencies.is_empty());
        assert!(c.unused.is_empty());
    }
}
