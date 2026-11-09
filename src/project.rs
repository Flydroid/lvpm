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
//! [dependencies.test]                # installed only when selected (--with test)
//! jki_lib_caraya = "*"
//!
//! [dependencies.ci]
//! include-groups = ["test"]         # everything in test, plus:
//! ni_vi_analyzer_tests = "1.2.0.3"
//! ```
//!
//! A dependency may also be a table, to say whether its install hooks run:
//! `wiresmith_technology_lib_g_cli = { version = "3.0.1.98", hooks = "skip" }`.
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

/// A dependency's `hooks` setting: whether lvpm runs the package's install
/// hooks (the VIs a package runs before and after its files are copied).
/// Unset, a venv runs none and a LabVIEW installation runs them all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hooks {
    Run,
    Skip,
}

impl Hooks {
    fn parse(s: &str) -> Result<Hooks> {
        match s {
            "run" => Ok(Hooks::Run),
            "skip" => Ok(Hooks::Skip),
            _ => bail!("hooks = {s:?}: expected \"run\" or \"skip\""),
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
    /// `[dependencies.<name>]` group tables, in file order.
    pub groups: Vec<(String, Group)>,
    /// Packages whose `hooks` is set, from every group.
    pub hooks: Vec<(String, Hooks)>,
}

/// One `[dependencies.<name>]` table: packages the project needs only
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
            hooks: Vec::new(),
        }
    }
}

impl Project {
    /// Every declared package name, including groups not selected for an install.
    pub fn declared_dependency_names(&self) -> impl Iterator<Item = &str> {
        self.dependencies.iter().map(|(name, _)| name.as_str()).chain(
            self.groups.iter().flat_map(|(_, group)| group.dependencies.iter().map(|(name, _)| name.as_str()))
        )
    }

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
    /// in both. Names are trimmed and empty ones dropped, so `--with "a, b"`
    /// and a trailing comma read as intended.
    pub fn select(&self, with: &[String], without: &[String], only: &[String]) -> Result<Selection> {
        fn clean(list: &[String]) -> Vec<&str> {
            list.iter().map(|s| s.trim()).filter(|s| !s.is_empty()).collect()
        }
        let (with, without, only) = (clean(with), clean(without), clean(only));
        let roots: Vec<&str> = match only.is_empty() {
            true => std::iter::once(MAIN_GROUP).chain(with.iter().copied()).collect(),
            false => only,
        };
        for g in roots.iter().chain(without.iter()).copied() {
            self.group_deps(g)?;
        }
        let mut order: Vec<&str> = Vec::new();
        for g in roots {
            self.expand(g, &mut Vec::new(), &mut order)?;
        }
        order.retain(|g| !without.contains(g));

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
        match value {
            toml::Value::Table(t) if !is_dependency_table(&t) => {
                let label = format!("[dependencies.{name}]");
                if name == MAIN_GROUP {
                    bail!("{label}: `{MAIN_GROUP}` is the name of [dependencies]; pick another");
                }
                let mut g = Group::default();
                for (key, value) in t {
                    if key == INCLUDE_KEY {
                        g.include = string_array(&label, &key, value)?;
                    } else {
                        let (c, hooks) = dependency(&label, &key, value)?;
                        if let Some(h) = hooks {
                            p.hooks.push((key.clone(), h));
                        }
                        g.dependencies.push((key, c));
                    }
                }
                p.groups.push((name, g));
            }
            value @ (toml::Value::String(_) | toml::Value::Table(_)) => {
                let (c, hooks) = dependency("[dependencies]", &name, value)?;
                if let Some(h) = hooks {
                    p.hooks.push((name.clone(), h));
                }
                p.dependencies.push((name, c));
            }
            other => bail!(
                "[dependencies] {name}: expected a quoted version, a {{ version = ... }} table or a group table, got {}",
                other.type_str()
            ),
        }
    }
    for (i, (name, _)) in p.hooks.iter().enumerate() {
        if let Some((_, h)) = p.hooks[..i].iter().find(|(n, _)| n.eq_ignore_ascii_case(name))
            && *h != p.hooks[i].1
        {
            bail!("{name} has hooks = \"run\" in one group and \"skip\" in another — make them agree");
        }
    }
    p.nipm = f.nipm.dependencies.into_iter().map(|(k, _)| k).collect();
    for (gname, g) in &p.groups {
        for inc in &g.include {
            if !p.groups.iter().any(|(n, _)| n == inc) {
                bail!("[dependencies.{gname}] {INCLUDE_KEY}: no group {inc:?}");
            }
        }
    }
    Ok(p)
}

/// The keys a dependency table may hold.
const DEPENDENCY_KEYS: [&str; 2] = ["version", "hooks"];

/// A table under `[dependencies]` is one dependency when it has a `version`
/// and nothing but [`DEPENDENCY_KEYS`]; any other table is a group. (So a
/// group cannot consist of packages named `version` and `hooks` alone.)
fn is_dependency_table(t: &toml::Table) -> bool {
    t.contains_key("version") && t.keys().all(|k| DEPENDENCY_KEYS.contains(&k.as_str()))
}

/// `name = "1.2.3"` or `name = { version = "1.2.3", hooks = "run" }`.
fn dependency(label: &str, name: &str, value: toml::Value) -> Result<(Constraint, Option<Hooks>)> {
    let at = || format!("{label} {name}");
    match value {
        toml::Value::String(s) => Ok((Constraint::parse(&s).with_context(at)?, None)),
        toml::Value::Table(t) if is_dependency_table(&t) => {
            let Some(toml::Value::String(v)) = t.get("version") else {
                bail!("{}: version must be a quoted version", at());
            };
            let c = Constraint::parse(v).with_context(at)?;
            let hooks = match t.get("hooks") {
                None => None,
                Some(toml::Value::String(h)) => Some(Hooks::parse(h).with_context(at)?),
                Some(other) => bail!("{}: hooks must be \"run\" or \"skip\", got {}", at(), other.type_str()),
            };
            Ok((c, hooks))
        }
        toml::Value::Table(t) => bail!(
            "{}: a dependency table takes {} and needs version; got {}",
            at(),
            DEPENDENCY_KEYS.join(", "),
            t.keys().cloned().collect::<Vec<_>>().join(", ")
        ),
        other => bail!("{}: expected a quoted version, got {}", at(), other.type_str()),
    }
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
    fn declared_dependency_names_include_unselected_groups() {
        let project = parse(
            "[dependencies]\nmain_package = \"*\"\n[dependencies.test]\ntest_package = \"*\"\n\
             [dependencies.build]\nbuild_package = \"*\"\n"
        ).unwrap();
        assert_eq!(
            project.declared_dependency_names().collect::<Vec<_>>(),
            ["main_package", "test_package", "build_package"]
        );
        assert_eq!(project.select(&[], &[], &[]).unwrap().dependencies.len(), 1);
    }

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

[dependencies.test]
jki_lib_caraya = "*"
oglib_error = ">=6.0"

[dependencies.ci]
include-groups = ["test"]
ni_vi_analyzer_tests = "1.2.0.3"

[dependencies.build]
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
            "[dependencies.a]\ninclude-groups = [\"b\"]\n[dependencies.b]\ninclude-groups = [\"a\"]\n",
        )
        .unwrap();
        let err = cyc.select(&strings(&["a"]), &none, &none).unwrap_err().to_string();
        assert!(err.contains("a -> b -> a"), "{err}");

        let clash = parse(
            "[dependencies]\noglib_error = \"6.0\"\n[dependencies.test]\nOGLIB_ERROR = \">=6.0\"\n",
        )
        .unwrap();
        let err = clash.select(&strings(&["test"]), &none, &none).unwrap_err().to_string();
        assert!(err.contains("in main") && err.contains("in test"), "{err}");

        let same = parse(
            "[dependencies]\noglib_error = \"6.0\"\n[dependencies.test]\noglib_error = \"6.0\"\n",
        )
        .unwrap();
        assert_eq!(names(&same.select(&strings(&["test"]), &none, &none).unwrap()), ["oglib_error"]);
    }

    #[test]
    fn malformed_groups_are_rejected_when_read() {
        assert!(parse("[dependencies.main]\nx = \"1\"\n").is_err(), "main is reserved");
        assert!(parse("[dependencies.t]\ninclude-groups = \"u\"\n").is_err(), "not an array");
        assert!(parse("[dependencies.t]\ninclude-groups = [\"u\"]\n").is_err(), "unknown include");
        assert!(parse("[dependencies.t]\nx = 1.0\n").is_err(), "unquoted version");
    }

    #[test]
    fn dependency_tables_set_hooks_and_are_told_apart_from_groups() {
        let p = parse(
            r#"
[dependencies]
ni_lib_advanced_http_client_api = { version = "1.1.0.6", hooks = "run" }
wiresmith_technology_lib_g_cli = { version = "3.0.1.98", hooks = "skip" }
oglib_error = { version = ">=6.0" }
delacor_lib_qmh = "7.1.2.1547"

[dependencies.test]
jki_lib_caraya = { version = "*", hooks = "run" }
oglib_array = "6.0.1.20"
"#,
        )
        .unwrap();
        let names: Vec<&str> = p.dependencies.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["ni_lib_advanced_http_client_api", "wiresmith_technology_lib_g_cli", "oglib_error", "delacor_lib_qmh"]);
        assert_eq!(p.dependencies[2].1, Constraint::AtLeast(Version::parse("6.0")));
        assert_eq!(p.groups.len(), 1, "an inline table with a version is a dependency, not a group");
        let hooks_for = |name: &str| p.hooks.iter().find(|(n, _)| n == name).map(|(_, h)| *h);
        assert_eq!(hooks_for("ni_lib_advanced_http_client_api"), Some(Hooks::Run));
        assert_eq!(hooks_for("wiresmith_technology_lib_g_cli"), Some(Hooks::Skip));
        assert_eq!(hooks_for("jki_lib_caraya"), Some(Hooks::Run), "groups count too");
        assert_eq!(hooks_for("delacor_lib_qmh"), None);

        assert!(parse("[dependencies]\nx = { version = \"1\", hooks = \"maybe\" }\n").is_err());
        assert!(parse("[dependencies]\nx = { version = 1 }\n").is_err(), "unquoted version");
        assert!(parse("[dependencies.t]\nx = { version = \"1\", source = \"local\" }\n").is_err(), "unknown key");
        assert!(
            parse("[dependencies]\nx = { version = \"1\", hooks = \"run\" }\n[dependencies.t]\nx = { version = \"1\", hooks = \"skip\" }\n").is_err(),
            "groups disagree"
        );
    }

    #[test]
    fn several_groups_per_flag_and_duplicates() {
        let p = parse(GROUPS).unwrap();
        let none: Vec<String> = Vec::new();

        let s = p.select(&strings(&["test", "build"]), &none, &none).unwrap();
        assert_eq!(s.groups, ["main", "test", "build"]);
        assert_eq!(names(&s), ["delacor_lib_qmh", "jki_lib_caraya", "oglib_error", "vipm_package_builder"]);

        let s = p.select(&strings(&["test", "test"]), &none, &none).unwrap();
        assert_eq!(s.groups, ["main", "test"]);
        let s = p.select(&strings(&["ci", "test"]), &none, &none).unwrap();
        assert_eq!(s.groups, ["main", "test", "ci"], "test once, before the group including it");
        let s = p.select(&strings(&["main"]), &none, &none).unwrap();
        assert_eq!(s.groups, ["main"]);

        let s = p.select(&strings(&["ci", "build"]), &strings(&["test", "build"]), &none).unwrap();
        assert_eq!(s.groups, ["main", "ci"]);
        assert_eq!(names(&s), ["delacor_lib_qmh", "ni_vi_analyzer_tests"]);
    }

    #[test]
    fn only_takes_several_groups_and_expands_includes() {
        let p = parse(GROUPS).unwrap();
        let none: Vec<String> = Vec::new();

        let s = p.select(&none, &none, &strings(&["main", "build"])).unwrap();
        assert_eq!(s.groups, ["main", "build"]);
        let s = p.select(&none, &none, &strings(&["build", "main"])).unwrap();
        assert_eq!(s.groups, ["build", "main"], "order as given");

        let s = p.select(&none, &none, &strings(&["ci"])).unwrap();
        assert_eq!(s.groups, ["test", "ci"]);
        assert_eq!(names(&s), ["jki_lib_caraya", "oglib_error", "ni_vi_analyzer_tests"]);
        assert!(p.select(&none, &none, &strings(&["nope"])).is_err());
    }

    #[test]
    fn without_a_group_that_was_not_selected_changes_nothing() {
        let p = parse(GROUPS).unwrap();
        let none: Vec<String> = Vec::new();
        let s = p.select(&none, &strings(&["build"]), &none).unwrap();
        assert_eq!(s.groups, ["main"]);
        let s = p.select(&none, &strings(&["main"]), &strings(&[])).unwrap();
        assert!(s.groups.is_empty() && s.dependencies.is_empty());
    }

    #[test]
    fn a_shared_include_is_expanded_once_in_a_stable_order() {
        let p = parse(
            r#"
[dependencies.d]
pkg_d = "*"
[dependencies.b]
include-groups = ["d"]
pkg_b = "*"
[dependencies.c]
include-groups = ["d"]
pkg_c = "*"
[dependencies.a]
include-groups = ["b", "c"]
pkg_a = "*"
"#,
        )
        .unwrap();
        let none: Vec<String> = Vec::new();
        let s = p.select(&strings(&["a"]), &none, &none).unwrap();
        assert_eq!(s.groups, ["main", "d", "b", "c", "a"]);
        assert_eq!(names(&s), ["pkg_d", "pkg_b", "pkg_c", "pkg_a"]);

        let s = p.select(&strings(&["a"]), &strings(&["d"]), &none).unwrap();
        assert_eq!(s.groups, ["main", "b", "c", "a"], "--without applies after includes");
    }

    #[test]
    fn names_are_trimmed_empty_entries_dropped_and_case_kept() {
        let p = parse(GROUPS).unwrap();
        let none: Vec<String> = Vec::new();

        let s = p.select(&strings(&[" test", "build ", ""]), &none, &none).unwrap();
        assert_eq!(s.groups, ["main", "test", "build"]);
        let s = p.select(&strings(&["ci"]), &strings(&[" test "]), &none).unwrap();
        assert_eq!(s.groups, ["main", "ci"]);
        let s = p.select(&none, &strings(&["  "]), &strings(&["", " "])).unwrap();
        assert_eq!(s.groups, ["main"], "an all-empty --only falls back to the default");

        let err = p.select(&strings(&["Test"]), &none, &none).unwrap_err().to_string();
        assert!(err.contains("\"Test\""), "group names are case-sensitive: {err}");
    }
}
