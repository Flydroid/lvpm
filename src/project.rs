//! The project manifest, `lvpm.toml`.
//!
//! ```toml
//! [project]
//! name = "NovaXC"
//! version = "0.1.0"
//! labview = "2025"                  # the oldest LabVIEW the project is meant for
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
//! [nipm.dependencies]               # NI Package Manager packages, reported only
//! ni-daqmx-labview-support = "26.0"
//!
//! [dependency-groups.test]          # installed only when selected (--with test)
//! jki_lib_caraya = "*"
//!
//! [dependency-groups.ci]
//! include-groups = ["test"]         # everything in test, plus:
//! ni_vi_analyzer_tests = "1.2.0.3"
//! ```
//!
//! `labview` is a minimum: a venv binds to that version or a newer one, never
//! an older one. Full reference, including `vipm.toml` compatibility:
//! `docs/manifest.md`.

use crate::version::Version;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::fmt;
use std::path::{Path, PathBuf};

/// The manifest's file name — what marks a directory as a project root.
pub const FILE_NAME: &str = "lvpm.toml";

/// The group name `--with` / `--without` / `--only` use for `[dependencies]`.
pub const MAIN_GROUP: &str = "main";

/// The reserved key in a group table naming the groups it includes.
const INCLUDE_KEY: &str = "include-groups";

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
    /// `[sources]`, in file order: a name and an index URL as written.
    pub sources: Vec<(String, String)>,
    /// Whether the public indexes are consulted too (`sources.defaults`).
    pub default_sources: bool,
    /// `[dependencies]`, in file order.
    pub dependencies: Vec<(String, Constraint)>,
    /// `[nipm.dependencies]` names, for reporting only.
    pub nipm: Vec<String>,
    /// `[dependency-groups.<name>]`, in file order.
    pub groups: Vec<(String, Group)>,
}

/// One `[dependency-groups.<name>]` table: packages the project needs only
/// sometimes (tests, CI, builds). Never installed unless selected.
#[derive(Debug, Default, PartialEq)]
pub struct Group {
    /// `include-groups`: groups whose packages this one takes as well.
    pub include: Vec<String>,
    /// The group's own packages, in file order, read like `[dependencies]`.
    pub dependencies: Vec<(String, Constraint)>,
}

/// What a manifest install takes once groups are chosen.
#[derive(Debug, PartialEq)]
pub struct Selection {
    /// The groups in effect, includes expanded, `main` first when present.
    pub groups: Vec<String>,
    /// Their packages merged in that order, each name once.
    pub dependencies: Vec<(String, Constraint)>,
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
            groups: Vec::new(),
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

    /// The packages a manifest install takes. By default only `main`
    /// (`[dependencies]`); `with` adds groups, `without` removes them after
    /// includes are expanded, and a non-empty `only` replaces the default.
    /// A package named by two selected groups must ask for the same version
    /// in both.
    pub fn select(&self, with: &[String], without: &[String], only: &[String]) -> Result<Selection> {
        let roots: Vec<&str> = match only.is_empty() {
            true => std::iter::once(MAIN_GROUP).chain(with.iter().map(String::as_str)).collect(),
            false => only.iter().map(String::as_str).collect(),
        };
        for g in roots.iter().copied().chain(without.iter().map(String::as_str)) {
            self.group_deps(g)?;
        }
        let mut order: Vec<&str> = Vec::new();
        for g in roots {
            self.expand(g, &mut Vec::new(), &mut order)?;
        }
        order.retain(|g| !without.iter().any(|w| w.as_str() == *g));

        let mut dependencies: Vec<(String, Constraint)> = Vec::new();
        let mut owner: Vec<&str> = Vec::new();
        for &g in &order {
            for (name, c) in self.group_deps(g)? {
                match dependencies.iter().position(|(n, _)| n.eq_ignore_ascii_case(name)) {
                    Some(i) if &dependencies[i].1 == c => {}
                    Some(i) => bail!(
                        "{name} is asked for as \"{name}{}\" in {} but \"{name}{c}\" in {g} — \
                         make them agree",
                        dependencies[i].1,
                        owner[i]
                    ),
                    None => {
                        dependencies.push((name.clone(), c.clone()));
                        owner.push(g);
                    }
                }
            }
        }
        Ok(Selection { groups: order.into_iter().map(String::from).collect(), dependencies })
    }

    fn group_deps(&self, name: &str) -> Result<&[(String, Constraint)]> {
        if name == MAIN_GROUP {
            return Ok(&self.dependencies);
        }
        match self.groups.iter().find(|(n, _)| n == name) {
            Some((_, g)) => Ok(&g.dependencies),
            None => bail!(
                "no dependency group {name:?} in the manifest (groups: {})",
                std::iter::once(MAIN_GROUP)
                    .chain(self.groups.iter().map(|(n, _)| n.as_str()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }

    /// Depth-first over `include-groups`: includes land before the group
    /// that names them, each group once; a group reached again while still
    /// on `stack` is a cycle.
    fn expand<'a>(&'a self, name: &'a str, stack: &mut Vec<&'a str>, order: &mut Vec<&'a str>) -> Result<()> {
        if stack.contains(&name) {
            stack.push(name);
            bail!("{INCLUDE_KEY} forms a cycle: {}", stack.join(" -> "));
        }
        if order.contains(&name) {
            return Ok(());
        }
        stack.push(name);
        if let Some((_, g)) = self.groups.iter().find(|(n, _)| n == name) {
            for inc in &g.include {
                self.expand(inc, stack, order)?;
            }
        }
        stack.pop();
        order.push(name);
        Ok(())
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
    #[serde(rename = "dependency-groups")]
    dependency_groups: toml::Table,
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
                bail!("[sources] {k}: expected an index URL string, got {}", other.type_str())
            }
        }
    }
    for (name, value) in f.dependencies {
        let c = dependency("[dependencies]", &name, value)?;
        p.dependencies.push((name, c));
    }
    p.nipm = f.nipm.dependencies.into_iter().map(|(k, _)| k).collect();
    for (gname, value) in f.dependency_groups {
        let label = format!("[dependency-groups.{gname}]");
        if gname == MAIN_GROUP {
            bail!("{label}: `{MAIN_GROUP}` is the name of [dependencies]; pick another");
        }
        let toml::Value::Table(t) = value else {
            bail!("{label}: expected a table, got {}", value.type_str());
        };
        let mut g = Group::default();
        for (key, value) in t {
            if key == INCLUDE_KEY {
                g.include = string_array(&label, &key, value)?;
            } else {
                let c = dependency(&label, &key, value)?;
                g.dependencies.push((key, c));
            }
        }
        p.groups.push((gname, g));
    }
    for (gname, g) in &p.groups {
        for inc in &g.include {
            if !p.groups.iter().any(|(n, _)| n == inc) {
                bail!("[dependency-groups.{gname}] {INCLUDE_KEY}: no group {inc:?}");
            }
        }
    }
    Ok(p)
}

fn dependency(label: &str, name: &str, value: toml::Value) -> Result<Constraint> {
    let toml::Value::String(s) = value else {
        bail!("{label} {name}: expected a quoted version, got {}", value.type_str());
    };
    Constraint::parse(&s).with_context(|| format!("{label} {name}"))
}

fn string_array(label: &str, key: &str, value: toml::Value) -> Result<Vec<String>> {
    let toml::Value::Array(items) = value else {
        bail!("{label} {key}: expected an array of group names, got {}", value.type_str());
    };
    items
        .into_iter()
        .map(|v| match v {
            toml::Value::String(s) => Ok(s),
            other => bail!("{label} {key}: expected group names, got {}", other.type_str()),
        })
        .collect()
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
    fn sources_must_be_hosted_index_folders() {
        let p = parse("[sources]\nnet = \"http://h/x\"\nsecure = \"https://h/y\"\n").unwrap();
        assert_eq!(p.resolved_sources().unwrap(), ["http://h/x", "https://h/y"]);

        let folder = parse("[sources]\nlocal = \"./Dependencies\"\n").unwrap();
        assert!(folder.resolved_sources().is_err(), "a path on disk is not a source");
    }

    const GROUPS: &str = r#"
[dependencies]
delacor_lib_qmh = "7.1.2.1547"

[dependency-groups.test]
jki_lib_caraya = "*"
oglib_error = ">=6.0"

[dependency-groups.ci]
include-groups = ["test"]
ni_vi_analyzer_tests = "1.2.0.3"

[dependency-groups.build]
vipm_package_builder = "*"
"#;

    fn names(s: &Selection) -> Vec<&str> {
        s.dependencies.iter().map(|(n, _)| n.as_str()).collect()
    }

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn groups_parse_in_file_order_with_includes() {
        let p = parse(GROUPS).unwrap();
        let gnames: Vec<&str> = p.groups.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(gnames, ["test", "ci", "build"]);
        assert_eq!(p.groups[1].1.include, ["test"]);
        assert_eq!(
            p.groups[1].1.dependencies,
            vec![("ni_vi_analyzer_tests".into(), Constraint::Exact(Version::parse("1.2.0.3")))]
        );
        assert_eq!(p.groups[0].1.dependencies[1].1, Constraint::AtLeast(Version::parse("6.0")));
    }

    #[test]
    fn selection_defaults_to_main_and_follows_with_without_only() {
        let p = parse(GROUPS).unwrap();
        let none: Vec<String> = Vec::new();

        let s = p.select(&none, &none, &none).unwrap();
        assert_eq!(s.groups, ["main"]);
        assert_eq!(names(&s), ["delacor_lib_qmh"]);

        let s = p.select(&strings(&["ci"]), &none, &none).unwrap();
        assert_eq!(s.groups, ["main", "test", "ci"]);
        assert_eq!(names(&s), ["delacor_lib_qmh", "jki_lib_caraya", "oglib_error", "ni_vi_analyzer_tests"]);

        let s = p.select(&strings(&["ci"]), &strings(&["test"]), &none).unwrap();
        assert_eq!(s.groups, ["main", "ci"]);

        let s = p.select(&none, &none, &strings(&["build"])).unwrap();
        assert_eq!(s.groups, ["build"]);
        assert_eq!(names(&s), ["vipm_package_builder"]);

        let s = p.select(&none, &strings(&["main"]), &none).unwrap();
        assert!(s.dependencies.is_empty());
    }

    #[test]
    fn unknown_groups_cycles_and_disagreements_are_errors() {
        let p = parse(GROUPS).unwrap();
        let none: Vec<String> = Vec::new();
        let err = p.select(&strings(&["dev"]), &none, &none).unwrap_err().to_string();
        assert!(err.contains("\"dev\"") && err.contains("main, test, ci, build"), "{err}");
        assert!(p.select(&none, &strings(&["nope"]), &none).is_err());

        let cyc = parse(
            "[dependency-groups.a]\ninclude-groups = [\"b\"]\n[dependency-groups.b]\ninclude-groups = [\"a\"]\n",
        )
        .unwrap();
        let err = cyc.select(&strings(&["a"]), &none, &none).unwrap_err().to_string();
        assert!(err.contains("a -> b -> a"), "{err}");

        let clash = parse(
            "[dependencies]\noglib_error = \"6.0\"\n[dependency-groups.test]\nOGLIB_ERROR = \">=6.0\"\n",
        )
        .unwrap();
        let err = clash.select(&strings(&["test"]), &none, &none).unwrap_err().to_string();
        assert!(err.contains("in main") && err.contains("in test"), "{err}");

        let same = parse(
            "[dependencies]\noglib_error = \"6.0\"\n[dependency-groups.test]\noglib_error = \"6.0\"\n",
        )
        .unwrap();
        assert_eq!(names(&same.select(&strings(&["test"]), &none, &none).unwrap()), ["oglib_error"]);
    }

    #[test]
    fn malformed_groups_are_rejected_when_read() {
        assert!(parse("[dependency-groups.main]\nx = \"1\"\n").is_err(), "main is reserved");
        assert!(parse("[dependency-groups]\ntest = \"x\"\n").is_err(), "group must be a table");
        assert!(parse("[dependency-groups.t]\ninclude-groups = \"u\"\n").is_err(), "not an array");
        assert!(parse("[dependency-groups.t]\ninclude-groups = [\"u\"]\n").is_err(), "unknown include");
        assert!(parse("[dependency-groups.t]\nx = 1.0\n").is_err(), "unquoted version");
    }
}
