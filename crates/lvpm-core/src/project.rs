//! The project manifest, `lvpm.toml`.
//!
//! ```toml
//! [project]
//! name = "NovaXC"
//! version = "0.1.0"
//! labview = ">=2025"                # the oldest LabVIEW the project is meant for
//!
//! [sources]                         # repositories beyond the public indexes
//! mirror = "http://host:8090/files" # a hosted index.vipr folder
//! defaults = true                   # false: only the sources listed here
//!
//! [dependencies]
//! delacor_lib_qmh = "7.1.2.1547"    # exactly this version
//! oglib_error = ">=6.0.1"           # the newest version satisfying the floor
//! jki_lib_caraya = "*"              # the newest version
//!
//! [dev-dependencies]                # test and CI tooling, same syntax
//! jki_lib_unit_test = "*"
//!
//! [nipm.dependencies]               # NI Package Manager packages, reported only
//! ni-daqmx-labview-support = "26.0"
//! ```
//!
//! `labview` is a minimum: a venv binds to that version or a newer one, never
//! an older one.

use crate::errors::LockError;
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

    /// Whether `version` meets this constraint.
    pub fn admits(&self, version: &Version) -> bool {
        match self {
            Constraint::Exact(v) => version == v,
            Constraint::AtLeast(v) => version >= v,
            Constraint::Any => true,
        }
    }
}

/// A package as a command names it: `name`, or `name@constraint`. The
/// constraint is as written, not yet read: [`Constraint::parse`] reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageArg {
    pub name: String,
    pub constraint: Option<String>,
}

impl PackageArg {
    /// The constraint, read: [`Constraint::Any`] when none was written.
    pub fn read_constraint(&self) -> Result<Constraint> {
        self.constraint
            .as_deref()
            .map_or(Ok(Constraint::Any), Constraint::parse)
    }
}

/// What is wrong with a [`PackageArg`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PackageArgError {
    /// No name: `""` or `@1.0`.
    #[error("{text:?} is not a package name or name@constraint")]
    EmptyName { text: String },
    /// `name@` with nothing after the `@`.
    #[error("a constraint is expected after \"@\" in \"{name}@\"")]
    EmptyConstraint { name: String },
}

impl PackageArg {
    /// Splits at the first `@`; white space around either part is dropped.
    pub fn parse(text: &str) -> Result<PackageArg, PackageArgError> {
        let (name, constraint) = match text.split_once('@') {
            Some((name, constraint)) => (name.trim(), Some(constraint.trim())),
            None => (text.trim(), None),
        };
        if name.is_empty() {
            return Err(PackageArgError::EmptyName {
                text: text.to_string(),
            });
        }
        if constraint == Some("") {
            return Err(PackageArgError::EmptyConstraint {
                name: name.to_string(),
            });
        }
        Ok(PackageArg {
            name: name.to_string(),
            constraint: constraint.map(str::to_string),
        })
    }
}

/// The year a manifest's `labview` names, `"2024"` for LabVIEW 2024. The one
/// reading of that value: locking, a headless install and a venv all use it.
pub fn labview_year(labview: &str) -> Result<u32, LockError> {
    match labview.trim().parse::<u32>() {
        Ok(year) if year >= 2000 => Ok(year),
        _ => Err(LockError::LabviewNotAYear {
            value: labview.to_string(),
        }),
    }
}

/// What a package name is compared by: ASCII lowercase, as VIPM compares them.
/// Letters outside ASCII are not folded, so every comparison of names agrees
/// with [`same_name`]. Names are never rewritten, only compared by their key.
pub fn name_key(name: &str) -> String {
    name.to_ascii_lowercase()
}

/// Whether two package names are one package: their [`name_key`]s are equal.
pub fn same_name(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
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
    /// `project.labview`, when stated, as the year its `">="` names: the
    /// oldest LabVIEW the project is meant for. A venv binds to this version or a newer one; a global
    /// install still takes its target from `--labview-version`.
    pub labview: Option<String>,
    /// `[sources]`, in file order: a name and an index URL as written.
    pub sources: Vec<(String, String)>,
    /// Whether the public indexes are consulted too (`sources.defaults`).
    pub default_sources: bool,
    /// `[dependencies]`, in file order.
    pub dependencies: Vec<(String, Constraint)>,
    /// `[dev-dependencies]`, in file order: the same syntax as `[dependencies]`.
    pub dev_dependencies: Vec<(String, Constraint)>,
    /// `[nipm.dependencies]` names, for reporting only.
    pub nipm: Vec<String>,
    /// `[distribution]`, as written: only `lvpm build` reads it, through
    /// `distribution::validate`.
    pub distribution: Option<toml::Table>,
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
            dev_dependencies: Vec::new(),
            nipm: Vec::new(),
            distribution: None,
        }
    }
}

impl Project {
    /// The `[sources]` as index arguments. Only hosted index folders: a path
    /// on disk is refused here rather than silently resolving nothing.
    pub fn resolved_sources(&self) -> Result<Vec<String>> {
        for (name, s) in &self.sources {
            if !s.starts_with("http://") && !s.starts_with("https://") {
                bail!("[sources] {name}: {s:?} is not an http:// or https:// index folder");
            }
        }
        Ok(self.sources.iter().map(|(_, s)| s.clone()).collect())
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
    #[serde(rename = "dev-dependencies")]
    dev_dependencies: toml::Table,
    nipm: NipmTable,
    distribution: Option<toml::Table>,
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
    start
        .ancestors()
        .map(|d| d.join(FILE_NAME))
        .find(|p| p.is_file())
}

pub fn read(path: &Path) -> Result<Project> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading manifest {}", path.display()))?;
    parse(&text).with_context(|| format!("parsing manifest {}", path.display()))
}

pub(crate) fn parse(text: &str) -> Result<Project> {
    let f: File = toml::from_str(text)?;
    let mut p = Project {
        name: f.project.name,
        version: f.project.version,
        labview: f
            .project
            .labview
            .as_deref()
            .map(labview_minimum)
            .transpose()?,
        distribution: f.distribution,
        ..Project::default()
    };
    for (key, value) in f.sources {
        match (key.as_str(), value) {
            ("defaults", toml::Value::Boolean(b)) => p.default_sources = b,
            (_, toml::Value::String(s)) => p.sources.push((key, s)),
            (k, other) => {
                bail!(
                    "[sources] {k}: expected an index URL string, got {}",
                    other.type_str()
                )
            }
        }
    }
    p.dependencies = parse_dependencies("dependencies", f.dependencies)?;
    p.dev_dependencies = parse_dependencies("dev-dependencies", f.dev_dependencies)?;
    p.nipm = f.nipm.dependencies.into_iter().map(|(k, _)| k).collect();
    Ok(p)
}

/// `project.labview` as written, `">=2024"`, to the year it starts at, `"2024"`:
/// what [`Project::labview`] holds, so every reader of it sees a plain year.
fn labview_minimum(text: &str) -> Result<String> {
    let is_year = |s: &str| s.len() == 4 && s.bytes().all(|b| b.is_ascii_digit()) && s >= "2000";
    let text = text.trim();
    match text.strip_prefix(">=").map(str::trim) {
        Some(year) if is_year(year) => Ok(year.to_string()),
        _ if is_year(text) => {
            bail!("project.labview = {text:?} is a minimum: write labview = \">={text}\"")
        }
        _ => bail!(
            "project.labview = {text:?} is not a minimum LabVIEW year: \
             write it like labview = \">=2024\""
        ),
    }
}

/// One dependency table, in file order. `table` names it in error messages.
fn parse_dependencies(table: &str, entries: toml::Table) -> Result<Vec<(String, Constraint)>> {
    entries
        .into_iter()
        .map(|(name, value)| {
            let toml::Value::String(s) = value else {
                bail!(
                    "[{table}] {name}: expected a quoted version, got {}",
                    value.type_str()
                );
            };
            let c = Constraint::parse(&s).with_context(|| format!("[{table}] {name}"))?;
            Ok((name, c))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_compared_by_their_ascii_lowercase_and_nothing_else() {
        assert_eq!(name_key("JKI_Lib_Caraya"), "jki_lib_caraya");
        // Not folded: a name that differs by a non-ASCII letter's case is another name.
        assert_ne!(name_key("Ärger"), name_key("ärger"));
        for (a, b) in [
            ("Beta", "beta"),
            ("Ärger", "ärger"),
            ("a", "b"),
            ("ß", "SS"),
        ] {
            assert_eq!(same_name(a, b), name_key(a) == name_key(b), "{a} {b}");
        }
    }

    #[test]
    fn a_package_arg_is_a_name_and_an_optional_constraint_and_neither_may_be_empty() {
        let arg = |text| PackageArg::parse(text).unwrap();
        assert_eq!(arg(" foo ").constraint, None);
        assert_eq!(arg("foo@>=1.0").constraint.as_deref(), Some(">=1.0"));
        assert_eq!(arg(" foo @ 1.0 "), arg("foo@1.0"));
        for text in ["", "  ", "@1.0", " @1.0"] {
            assert_eq!(
                PackageArg::parse(text),
                Err(PackageArgError::EmptyName { text: text.into() }),
                "{text:?}"
            );
        }
        for text in ["foo@", "foo@  "] {
            assert_eq!(
                PackageArg::parse(text),
                Err(PackageArgError::EmptyConstraint { name: "foo".into() }),
                "{text:?}"
            );
        }
    }

    #[test]
    fn labview_is_a_minimum_year_and_every_other_form_is_refused_by_key() {
        let labview = |value: &str| parse(&format!("[project]\nlabview = {value:?}\n"));
        assert_eq!(labview(">=2024").unwrap().labview.as_deref(), Some("2024"));
        assert_eq!(
            labview(" >= 2024 ").unwrap().labview.as_deref(),
            Some("2024")
        );
        let plain = labview("2024").unwrap_err().to_string();
        assert!(
            plain.contains("project.labview") && plain.contains("\">=2024\""),
            "{plain}"
        );
        for value in [
            ">2024", "^2024", "<=2024", ">=24", ">=2024.1", ">=", "", ">=1999", "soon",
        ] {
            let why = labview(value).unwrap_err().to_string();
            assert!(why.contains("project.labview"), "{value:?}: {why}");
        }
    }

    #[test]
    fn reads_every_table_in_file_order() {
        let p = parse(
            r#"
[project]
name = "NovaXC"
version = "0.1.0"
labview = ">=2025"

[sources]
mirror = "http://host:8090/files"
backup = "https://host/other"

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
                ("mirror".into(), "http://host:8090/files".into()),
                ("backup".into(), "https://host/other".into())
            ]
        );
        assert!(p.default_sources);
        let names: Vec<&str> = p.dependencies.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            [
                "oglib_array",
                "lava_lib_tree_control_api",
                "oglib_error",
                "jki_lib_caraya"
            ]
        );
        assert_eq!(
            p.dependencies[0].1,
            Constraint::Exact(Version::parse("6.0.1.20"))
        );
        assert_eq!(
            p.dependencies[1].1,
            Constraint::Exact(Version::parse("1.0.1-1"))
        );
        assert_eq!(
            p.dependencies[2].1,
            Constraint::AtLeast(Version::parse("6.0.1"))
        );
        assert_eq!(p.dependencies[3].1, Constraint::Any);
        assert_eq!(p.nipm, ["ni-daqmx-labview-support"]);
    }

    #[test]
    fn a_constraint_admits_its_own_bound() {
        let v = |s: &str| Version::parse(s);
        assert!(Constraint::AtLeast(v("2.0")).admits(&v("2.0")));
        assert!(!Constraint::AtLeast(v("2.0")).admits(&v("1.9")));
        assert!(Constraint::Exact(v("1.0")).admits(&v("1.0")));
        assert!(!Constraint::Exact(v("1.0")).admits(&v("1.1")));
        assert!(Constraint::Any.admits(&v("0.1")));
        let arg = |s: &str| PackageArg::parse(s).unwrap().read_constraint();
        assert_eq!(arg("a").unwrap(), Constraint::Any);
        assert!(arg("a@<2").is_err());
    }

    #[test]
    fn constraints_parse_the_three_forms_and_refuse_the_rest() {
        let v = |s: &str| Version::parse(s);
        assert_eq!(
            Constraint::parse("6.0.1.20").unwrap(),
            Constraint::Exact(v("6.0.1.20"))
        );
        assert_eq!(
            Constraint::parse(">=6.0.1").unwrap(),
            Constraint::AtLeast(v("6.0.1"))
        );
        assert_eq!(
            Constraint::parse(">= 6.0.1 ").unwrap(),
            Constraint::AtLeast(v("6.0.1"))
        );
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
        assert!(
            parse("[dependencies]\noglib_array = 6.0\n").is_err(),
            "unquoted version is a float"
        );
        assert!(
            parse("[dependencies]\noglib_array = [\"6.0\"]\n").is_err(),
            "wrong type"
        );
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
    fn sources_must_be_hosted_index_folders() {
        let p = parse("[sources]\nnet = \"http://h/x\"\nsecure = \"https://h/y\"\n").unwrap();
        assert_eq!(p.resolved_sources().unwrap(), ["http://h/x", "https://h/y"]);

        let folder = parse("[sources]\nlocal = \"./Dependencies\"\n").unwrap();
        assert!(
            folder.resolved_sources().is_err(),
            "a path on disk is not a source"
        );
    }

    #[test]
    fn dev_dependencies_use_the_dependency_syntax() {
        let p = parse(
            "[dependencies]\nalpha = \"1.0\"\n\n[dev-dependencies]\ncaraya = \"*\"\nunit = \">=2.1\"\n",
        )
        .unwrap();
        assert_eq!(p.dependencies.len(), 1, "the tables stay apart");
        assert_eq!(
            p.dev_dependencies,
            vec![
                ("caraya".to_string(), Constraint::Any),
                (
                    "unit".to_string(),
                    Constraint::AtLeast(Version::parse("2.1"))
                )
            ]
        );
        let none = parse("[project]\nname = \"x\"\n").unwrap();
        assert!(none.dev_dependencies.is_empty());
    }

    #[test]
    fn an_unsupported_dev_dependency_constraint_names_the_entry_and_the_constraint() {
        let why = format!(
            "{:#}",
            parse("[dev-dependencies]\ncaraya = \"~1.0\"\n").unwrap_err()
        );
        assert!(why.contains("[dev-dependencies] caraya"), "{why}");
        assert!(why.contains("~1.0"), "{why}");
        assert!(parse("[dev-dependencies]\ncaraya = 1.0\n").is_err());
    }
}
