//! The project manifest — VIPM's `vipm.toml`, read for its dependency list.
//!
//! Only the parts lvpm can act on are modelled: `[dependencies]`, which is a
//! flat table of `package = "exact.version"`, and the `[nipm.dependencies]`
//! table, which is recorded solely so the caller can say out loud that those
//! are NI Package Manager packages and lvpm does not install them.
//!
//! Hand-parsed rather than pulled through a TOML crate: the file shape is one
//! table of string-to-string, and every dependency lvpm keeps costs the whole
//! project a supply chain.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

use crate::version::Version;

/// The manifest's file name — what marks a directory as a project root.
pub const FILE_NAME: &str = "vipm.toml";

#[derive(Debug, Default, PartialEq)]
pub struct Project {
    /// `project.name`, when stated.
    pub name: Option<String>,
    /// `project.labview-version`, when stated: the oldest LabVIEW the project
    /// is meant for. A venv binds to this version or a newer one; a global
    /// install still takes its target from `--labview-version`.
    pub labview_version: Option<String>,
    /// `[dependencies]`, in file order. VIPM writes exact versions.
    pub dependencies: Vec<(String, Version)>,
    /// `[nipm.dependencies]` names, for reporting only.
    pub nipm: Vec<String>,
}

/// The nearest `vipm.toml` at or above `start` — a project is wherever its
/// manifest is, the way cargo finds `Cargo.toml`.
pub fn find_manifest(start: &Path) -> Option<PathBuf> {
    start.ancestors().map(|d| d.join(FILE_NAME)).find(|p| p.is_file())
}

pub fn read(path: &Path) -> Result<Project> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading manifest {}", path.display()))?;
    parse(&text).with_context(|| format!("parsing manifest {}", path.display()))
}

pub fn parse(text: &str) -> Result<Project> {
    let mut p = Project::default();
    let mut section = String::new();
    for (i, raw) in text.lines().enumerate() {
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(head) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = head.trim().to_ascii_lowercase();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            bail!("line {}: expected key = value, got {line:?}", i + 1);
        };
        let key = key.trim().trim_matches('"');
        let value = unquote(value.trim())
            .with_context(|| format!("line {}: {key} has an unquoted value", i + 1))?;
        match section.as_str() {
            "project" => match key {
                "name" => p.name = Some(value.to_string()),
                "labview-version" => p.labview_version = Some(value.to_string()),
                _ => {} // other project keys are none of lvpm's business
            },
            "dependencies" => p.dependencies.push((key.to_string(), Version::parse(value))),
            "nipm.dependencies" => p.nipm.push(key.to_string()),
            _ => {}
        }
    }
    Ok(p)
}

/// Drop a trailing `#` comment, but not a `#` inside a quoted value.
fn strip_comment(line: &str) -> &str {
    let mut in_quotes = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_quotes = !in_quotes,
            '#' if !in_quotes => return &line[..i],
            _ => {}
        }
    }
    line
}

fn unquote(v: &str) -> Result<&str> {
    v.strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .context("expected a double-quoted string")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_dependency_table_in_order() {
        let p = parse(
            r#"
[project]
name = "NovaXC"
version = "0.1.0"
labview-version = "2025"

[dependencies]
oglib_array = "6.0.1.20"
lava_lib_tree_control_api = "1.0.1-1"   # odd but real

[nipm]

[nipm.dependencies]
ni-daqmx-labview-support = "26.0.0.49434-0+f282"
"#,
        )
        .unwrap();
        assert_eq!(p.name.as_deref(), Some("NovaXC"));
        assert_eq!(p.labview_version.as_deref(), Some("2025"));
        let names: Vec<&str> = p.dependencies.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["oglib_array", "lava_lib_tree_control_api"]);
        assert_eq!(p.dependencies[0].1.raw, "6.0.1.20");
        assert_eq!(p.dependencies[1].1.raw, "1.0.1-1");
        assert_eq!(p.nipm, ["ni-daqmx-labview-support"]);
    }

    #[test]
    fn rejects_a_line_it_cannot_read() {
        assert!(parse("[dependencies]\noglib_array\n").is_err());
        assert!(parse("[dependencies]\noglib_array = 6.0.1.20\n").is_err(), "unquoted version");
    }

    #[test]
    fn a_manifest_with_no_dependencies_is_not_an_error() {
        assert_eq!(parse("[project]\nname = \"x\"\n").unwrap().dependencies, vec![]);
    }
}
