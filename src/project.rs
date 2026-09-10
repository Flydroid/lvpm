//! The project manifest, `lvpm.toml`.
//!
//! ```toml
//! [project]
//! name = "NovaXC"
//! version = "0.1.0"
//! labview = "2025"                  # the oldest LabVIEW the project is meant for
//!
//! [sources]                         # repositories beyond the public indexes
//! local = "./Dependencies"          # a folder of .vip files, or an index URL
//! defaults = true                   # false: only the sources listed here
//!
//! [dependencies]
//! delacor_lib_qmh = "7.1.2.1547"    # exactly this version
//! oglib_error = ">=6.0.1"           # the newest version satisfying the floor
//! jki_lib_caraya = "*"              # the newest version
//!
//! [nipm.dependencies]               # NI Package Manager packages, reported only
//! ni-daqmx-labview-support = "26.0"
//! ```
//!
//! `[sources]` folders are relative to the manifest, so a project's own
//! `Dependencies` directory works from any working directory, and on any
//! machine that has the checkout. `labview` is a minimum: a venv binds to that
//! version or a newer one, never an older one.

use crate::version::Version;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::fmt;
use std::path::{Path, PathBuf};

/// The manifest's file name — what marks a directory as a project root.
pub const FILE_NAME: &str = "lvpm.toml";

/// What a dependency line asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum Constraint {
    /// `"1.2.3.4"` — this version and no other.
    Exact(Version),
    /// `">=1.2"` — the newest version not below this one.
    AtLeast(Version),
    /// `"*"` — the newest version there is.
    Any,
}

impl Constraint {
    pub fn parse(s: &str) -> Result<Constraint> {
        let s = s.trim();
        Ok(match s {
            "" | "*" => Constraint::Any,
            _ if s.starts_with(">=") => Constraint::AtLeast(Version::parse(s[2..].trim())),
            _ if s.starts_with(['<', '>', '=', '~', '^']) => bail!(
                "unsupported version constraint {s:?} — use an exact version, \">=x.y\" or \"*\""
            ),
            _ => Constraint::Exact(Version::parse(s)),
        })
    }
}

impl fmt::Display for Constraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Constraint::Exact(v) => write!(f, "@{}", v.raw),
            Constraint::AtLeast(v) => write!(f, " >={}", v.raw),
            Constraint::Any => Ok(()),
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct Project {
    /// `project.name`, when stated.
    pub name: Option<String>,
    /// `project.version`, when stated. Not acted on.
    pub version: Option<String>,
    /// `project.labview`, when stated: the oldest LabVIEW the project is
    /// meant for. A venv binds to this version or a newer one; a global
    /// install still takes its target from `--labview-version`.
    pub labview: Option<String>,
    /// `[sources]`, in file order: a name, and a URL or a folder as written.
    pub sources: Vec<(String, String)>,
    /// Whether the public indexes are consulted too (`sources.defaults`).
    pub default_sources: bool,
    /// `[dependencies]`, in file order.
    pub dependencies: Vec<(String, Constraint)>,
    /// `[nipm.dependencies]` names, for reporting only.
    pub nipm: Vec<String>,
}

impl Default for Project {
    fn default() -> Self {
        Project {
            name: None,
            version: None,
            labview: None,
            sources: Vec::new(),
            default_sources: true,
            dependencies: Vec::new(),
            nipm: Vec::new(),
        }
    }
}

impl Project {
    /// The `[sources]` as `--repo` arguments: URLs as written, folders made
    /// absolute against the manifest's directory. A folder that does not
    /// exist is an error here rather than a silent miss in the index.
    pub fn resolved_sources(&self, manifest_dir: &Path) -> Result<Vec<String>> {
        let mut out = Vec::new();
        for (name, s) in &self.sources {
            if s.starts_with("http://") || s.starts_with("https://") {
                out.push(s.clone());
                continue;
            }
            let p = manifest_dir.join(s);
            if !p.is_dir() {
                bail!("[sources] {name}: {} is not a folder", p.display());
            }
            out.push(std::path::absolute(&p)?.to_string_lossy().into_owned());
        }
        Ok(out)
    }
}

/// The file as TOML sees it. Tables stay tables so their order survives and
/// so a wrong value type is reported by key rather than as a shape mismatch.
#[derive(Deserialize, Default)]
#[serde(default)]
struct File {
    project: ProjectTable,
    sources: toml::Table,
    dependencies: toml::Table,
    nipm: NipmTable,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ProjectTable {
    name: Option<String>,
    version: Option<String>,
    labview: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct NipmTable {
    dependencies: toml::Table,
}

/// The nearest `lvpm.toml` at or above `start` — a project is wherever its
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
    let f: File = toml::from_str(text)?;
    let mut p = Project {
        name: f.project.name,
        version: f.project.version,
        labview: f.project.labview,
        ..Project::default()
    };
    for (key, value) in f.sources {
        match (key.as_str(), value) {
            ("defaults", toml::Value::Boolean(b)) => p.default_sources = b,
            (_, toml::Value::String(s)) => p.sources.push((key, s)),
            (k, other) => {
                bail!("[sources] {k}: expected a URL or folder string, got {}", other.type_str())
            }
        }
    }
    for (name, value) in f.dependencies {
        let toml::Value::String(s) = value else {
            bail!("[dependencies] {name}: expected a quoted version, got {}", value.type_str());
        };
        let c = Constraint::parse(&s).with_context(|| format!("[dependencies] {name}"))?;
        p.dependencies.push((name, c));
    }
    p.nipm = f.nipm.dependencies.into_iter().map(|(k, _)| k).collect();
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_every_table_in_file_order() {
        let p = parse(
            r#"
[project]
name = "NovaXC"
version = "0.1.0"
labview = "2025"

[sources]
local = "./Dependencies"
mirror = "http://host:8090/files"

[dependencies]
oglib_array = "6.0.1.20"
lava_lib_tree_control_api = "1.0.1-1"   # odd but real
oglib_error = ">=6.0.1"
jki_lib_caraya = "*"

[nipm]

[nipm.dependencies]
ni-daqmx-labview-support = "26.0.0.49434-0+f282"
"#,
        )
        .unwrap();
        assert_eq!(p.name.as_deref(), Some("NovaXC"));
        assert_eq!(p.labview.as_deref(), Some("2025"));
        assert_eq!(
            p.sources,
            vec![
                ("local".into(), "./Dependencies".into()),
                ("mirror".into(), "http://host:8090/files".into())
            ]
        );
        assert!(p.default_sources);
        let names: Vec<&str> = p.dependencies.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["oglib_array", "lava_lib_tree_control_api", "oglib_error", "jki_lib_caraya"]);
        assert_eq!(p.dependencies[0].1, Constraint::Exact(Version::parse("6.0.1.20")));
        assert_eq!(p.dependencies[1].1, Constraint::Exact(Version::parse("1.0.1-1")));
        assert_eq!(p.dependencies[2].1, Constraint::AtLeast(Version::parse("6.0.1")));
        assert_eq!(p.dependencies[3].1, Constraint::Any);
        assert_eq!(p.nipm, ["ni-daqmx-labview-support"]);
    }

    #[test]
    fn constraints_parse_the_three_forms_and_refuse_the_rest() {
        let v = |s: &str| Version::parse(s);
        assert_eq!(Constraint::parse("6.0.1.20").unwrap(), Constraint::Exact(v("6.0.1.20")));
        assert_eq!(Constraint::parse(">=6.0.1").unwrap(), Constraint::AtLeast(v("6.0.1")));
        assert_eq!(Constraint::parse(">= 6.0.1 ").unwrap(), Constraint::AtLeast(v("6.0.1")));
        assert_eq!(Constraint::parse("*").unwrap(), Constraint::Any);
        assert_eq!(Constraint::parse("").unwrap(), Constraint::Any);
        assert!(Constraint::parse("~1.2").is_err());
        assert!(Constraint::parse("<2").is_err());
        assert!(Constraint::parse("=1.2").is_err());
        assert_eq!(Constraint::parse("1.2").unwrap().to_string(), "@1.2");
        assert_eq!(Constraint::parse(">=1.2").unwrap().to_string(), " >=1.2");
        assert_eq!(Constraint::parse("*").unwrap().to_string(), "");
    }

    #[test]
    fn rejects_what_it_cannot_read() {
        assert!(parse("[dependencies]\noglib_array\n").is_err(), "not TOML");
        assert!(parse("[dependencies]\noglib_array = 6.0\n").is_err(), "unquoted version is a float");
        assert!(parse("[dependencies]\noglib_array = [\"6.0\"]\n").is_err(), "wrong type");
        assert!(parse("[sources]\nlocal = 3\n").is_err());
    }

    #[test]
    fn sources_defaults_switch_and_missing_tables_are_fine() {
        let p = parse("[project]\nname = \"x\"\n").unwrap();
        assert_eq!(p.dependencies, vec![]);
        assert!(p.default_sources);
        let p = parse("[sources]\ndefaults = false\n").unwrap();
        assert!(!p.default_sources);
        assert!(p.sources.is_empty());
    }

    #[test]
    fn source_folders_resolve_against_the_manifest_dir() {
        let dir = std::env::temp_dir().join(format!("lvpm-project-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Dependencies")).unwrap();
        let p = parse("[sources]\nlocal = \"./Dependencies\"\nnet = \"http://h/x\"\n").unwrap();
        let r = p.resolved_sources(&dir).unwrap();
        assert_eq!(r.len(), 2);
        assert!(Path::new(&r[0]).is_absolute() && r[0].ends_with("Dependencies"), "{}", r[0]);
        assert_eq!(r[1], "http://h/x");
        let missing = parse("[sources]\nlocal = \"./nope\"\n").unwrap();
        assert!(missing.resolved_sources(&dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
