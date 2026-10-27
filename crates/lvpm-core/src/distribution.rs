//! `[distribution]`: what `lvpm build` packages, and how it names it.
//!
//! ```toml
//! [distribution]
//! top-level-library = "HSE Logger.lvlib"  # relative to lvpm.toml, inside its folder
//! kinds   = ["source"]                    # the only kind this version builds
//! formats = ["vip"]                       # the only format this version writes
//!
//! [distribution.vip]
//! name         = "hse_lib_hse_logger"     # default: [project] name
//! vendor       = "Hampel Software Engineering"
//! display-name = "HSE Logger"
//! install-dir  = "vi.lib/Hampel Software Engineering/HSE Logger"  # default: vi.lib/<vendor>/<display-name>
//!
//! [distribution.vip.dependency-names]     # scoped name -> VIPM name
//! "@hexagon/lib-a" = "hexagon_lib_a"
//! ```

use crate::project::{Constraint, Project};
use crate::spec::{FileGroup, Spec};
use anyhow::{Result, bail};
use std::path::{Component, Path, PathBuf};

/// A manifest checked for `lvpm build`: every key it needs, present and valid.
#[derive(Debug, PartialEq)]
pub struct Distribution {
    /// `top-level-library`, below the project folder.
    pub library: PathBuf,
    /// The package name: `[distribution.vip] name`, else `[project] name`.
    pub name: String,
    /// `[project] version` in four parts.
    pub version: String,
    pub vendor: String,
    pub display_name: String,
    /// Where the members go, below the LabVIEW folder, with forward slashes.
    pub install_dir: String,
    /// `[project] labview`'s year.
    pub labview: u32,
    /// `[dependencies]` as `Requires` entries, in file order.
    pub requires: Vec<String>,
}

const DISTRIBUTION_KEYS: &[&str] = &["top-level-library", "kinds", "formats", "vip"];
const VIP_KEYS: &[&str] = &[
    "name",
    "vendor",
    "display-name",
    "install-dir",
    "dependency-names",
];

/// `p`'s `[distribution]`, checked against the project folder `root`. Each
/// error names the full key it is about.
pub fn validate(p: &Project, root: &Path) -> Result<Distribution> {
    let Some(dist) = &p.distribution else {
        bail!(
            "lvpm.toml has no [distribution] table: lvpm build needs one naming the top-level-library"
        );
    };
    unknown_keys("distribution", dist, DISTRIBUTION_KEYS)?;
    only(dist, "kinds", "source")?;
    only(dist, "formats", "vip")?;

    let empty = toml::Table::new();
    let vip = match dist.get("vip") {
        None => &empty,
        Some(toml::Value::Table(t)) => t,
        Some(v) => bail!("distribution.vip: expected a table, got {}", v.type_str()),
    };
    unknown_keys("distribution.vip", vip, VIP_KEYS)?;

    let library = top_level_library(dist, root)?;
    let vendor = required(vip, "distribution.vip", "vendor")?;
    let display_name = required(vip, "distribution.vip", "display-name")?;
    // The default goes through the same check: a vendor of `..\x` must not leave vi.lib.
    let install_dir = match string(vip, "distribution.vip", "install-dir")? {
        Some(d) => d,
        None => format!("vi.lib/{vendor}/{display_name}"),
    }
    .replace('\\', "/")
    .trim_matches('/')
    .to_string();
    if install_dir.is_empty()
        || install_dir.contains(':')
        || install_dir.split('/').any(|s| s == "..")
    {
        bail!(
            "distribution.vip.install-dir = {install_dir:?}: must be a relative path below the LabVIEW folder, \
             like \"vi.lib/Vendor/Lib\""
        );
    }

    let name = match string(vip, "distribution.vip", "name")? {
        Some(n) => n,
        None => match &p.name {
            Some(n) if n.contains('@') => bail!(
                "distribution.vip.name: the project name {n:?} is scoped, which VIPM cannot name a package; \
                 set [distribution.vip] name"
            ),
            Some(n) => n.clone(),
            None => bail!("project.name: lvpm build needs the package name"),
        },
    };
    // The name becomes the file name in dist/: nothing that leaves the folder.
    if name.contains("..")
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-.".contains(c))
    {
        bail!(
            "distribution.vip.name = {name:?}: a package name takes letters, digits, '_', '-' and '.', \
             like \"vendor_lib_name\""
        );
    }
    let Some(version) = &p.version else {
        bail!("project.version: lvpm build needs the package version");
    };
    let version = four_parts(version)?;
    let Some(labview) = &p.labview else {
        bail!("project.labview: lvpm build needs the oldest LabVIEW, like labview = \">=2016\"");
    };
    let labview = crate::project::labview_year(labview)?;

    let names = match vip.get("dependency-names") {
        None => &empty,
        Some(toml::Value::Table(t)) => t,
        Some(v) => bail!(
            "distribution.vip.dependency-names: expected a table, got {}",
            v.type_str()
        ),
    };
    let requires: Vec<String> = p
        .dependencies
        .iter()
        .map(|(dep, c)| {
            let vip_name = match names.get(dep) {
                Some(toml::Value::String(s)) => s.clone(),
                Some(v) => bail!(
                    "distribution.vip.dependency-names: {dep:?} expected a string, got {}",
                    v.type_str()
                ),
                None if dep.contains('@') => bail!(
                    "distribution.vip.dependency-names: the dependency {dep:?} is scoped and has no VIPM name here"
                ),
                None => dep.clone(),
            };
            // An exact version is written as a minimum, like the published
            // pilots: a library pinning exact versions makes consumers unsolvable.
            Ok(match c {
                Constraint::Exact(v) | Constraint::AtLeast(v) => format!("{vip_name}>={}", v.raw),
                Constraint::Any => vip_name,
            })
        })
        .collect::<Result<_>>()?;
    for (key, value) in [
        ("distribution.vip.vendor", &vendor),
        ("distribution.vip.display-name", &display_name),
        ("distribution.vip.install-dir", &install_dir),
    ] {
        spec_safe(key, value)?;
    }
    for r in &requires {
        spec_safe("dependencies", r)?;
        if r.contains(',') {
            bail!("dependencies: {r:?} holds a ',', which would split the package's Requires list");
        }
    }

    Ok(Distribution {
        library,
        name,
        version,
        vendor,
        display_name,
        install_dir,
        labview,
        requires,
    })
}

impl Distribution {
    /// The package file's name, `<name>-<version>.vip`.
    pub fn file_name(&self) -> String {
        format!("{}-{}.vip", self.name, self.version)
    }

    /// The spec for a package holding `files`, each below the LabVIEW folder.
    pub fn spec(&self, files: Vec<String>) -> Spec {
        Spec {
            name: self.name.clone(),
            version: self.version.clone(),
            display_name: Some(self.display_name.clone()),
            description: Default::default(),
            file_groups: vec![FileGroup {
                index: 0,
                target_dir: "<application>".into(),
                replace_mode: "Always".into(),
                files,
            }],
            script_vis: Vec::new(),
            requires: (!self.requires.is_empty()).then(|| self.requires.join(",")),
            lv_gate: Some(format!("LabVIEW>={}.0", self.labview - 2000)),
        }
    }
}

/// A value written between quotes in the spec: no quote, line break or other
/// control character, which would end the value and start new spec keys.
pub fn spec_safe(key: &str, value: &str) -> Result<()> {
    if value.chars().any(|c| c == '"' || c.is_control()) {
        bail!("{key} = {value:?}: a quote or control character cannot go into the package's spec");
    }
    Ok(())
}

fn unknown_keys(table: &str, t: &toml::Table, known: &[&str]) -> Result<()> {
    match t.keys().find(|k| !known.contains(&k.as_str())) {
        Some(k) => bail!(
            "{table}.{k}: lvpm build does not support this key yet, a later version adds it; it reads {}",
            known.join(", ")
        ),
        None => Ok(()),
    }
}

/// `kinds` / `formats`: absent, or exactly `[<value>]`.
fn only(t: &toml::Table, key: &str, value: &str) -> Result<()> {
    match t.get(key) {
        None => Ok(()),
        Some(toml::Value::Array(a)) if a.len() == 1 && a[0].as_str() == Some(value) => Ok(()),
        Some(v) => bail!("distribution.{key} = {v}: this version builds only [\"{value}\"]"),
    }
}

fn string(t: &toml::Table, table: &str, key: &str) -> Result<Option<String>> {
    match t.get(key) {
        None => Ok(None),
        Some(toml::Value::String(s)) if !s.trim().is_empty() => Ok(Some(s.trim().to_string())),
        Some(v) => bail!("{table}.{key}: expected a non-empty string, got {v}"),
    }
}

fn required(t: &toml::Table, table: &str, key: &str) -> Result<String> {
    match string(t, table, key)? {
        Some(s) => Ok(s),
        None => bail!("{table}.{key}: lvpm build needs it"),
    }
}

fn top_level_library(dist: &toml::Table, root: &Path) -> Result<PathBuf> {
    const KEY: &str = "distribution.top-level-library";
    let Some(rel) = string(dist, "distribution", "top-level-library")? else {
        bail!(
            "{KEY}: lvpm build needs the library to package, like top-level-library = \"My Lib.lvlib\""
        );
    };
    let path = Path::new(&rel);
    let leaves = path.has_root()
        || path
            .components()
            .any(|c| matches!(c, Component::Prefix(_) | Component::ParentDir))
        || rel.contains(':')
        || rel.contains('\\');
    if leaves {
        bail!(
            "{KEY} = {rel:?}: must be a path inside the project folder, relative to lvpm.toml, with forward slashes"
        );
    }
    if !root.join(path).is_file() {
        bail!(
            "{KEY} = {rel:?}: there is no such file in {}",
            root.display()
        );
    }
    Ok(path.to_path_buf())
}

/// `2.1.0` → `2.1.0.0`; four numeric parts stay as they are.
fn four_parts(v: &str) -> Result<String> {
    let parts: Vec<&str> = v.split('.').collect();
    let numeric = parts
        .iter()
        .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    match parts.len() {
        3 if numeric => Ok(format!("{v}.0")),
        4 if numeric => Ok(v.to_string()),
        _ => bail!("project.version = {v:?}: a package version is 3 or 4 numbers, like \"1.2.3\""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        let d = std::env::temp_dir().join(format!("lvpm-distribution-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("Lib.lvlib"), "<Library/>").unwrap();
        d
    }

    const GOOD: &str = r#"
[project]
name = "demo_lib"
version = "1.2.3"
labview = ">=2016"

[dependencies]
a = "1.0"
b = ">=2.0"
c = "*"
"@hexagon/lib-a" = "3.0"

[distribution]
top-level-library = "Lib.lvlib"

[distribution.vip]
vendor = "Vendor"
display-name = "Demo Lib"

[distribution.vip.dependency-names]
"@hexagon/lib-a" = "hexagon_lib_a"
"#;

    fn check(text: &str) -> Result<Distribution> {
        validate(&crate::project::parse(text).unwrap(), &root())
    }

    #[test]
    fn a_complete_manifest_gives_names_version_install_dir_and_requires() {
        let d = check(GOOD).unwrap();
        assert_eq!(d.file_name(), "demo_lib-1.2.3.0.vip");
        assert_eq!(d.install_dir, "vi.lib/Vendor/Demo Lib");
        assert_eq!(d.requires, ["a>=1.0", "b>=2.0", "c", "hexagon_lib_a>=3.0"]);
        assert_eq!(d.library, Path::new("Lib.lvlib"));
    }

    #[test]
    fn each_problem_names_its_key() {
        let cases = [
            (
                GOOD.replace(
                    "[distribution]\ntop-level-library = \"Lib.lvlib\"\n",
                    "[distribution]\n",
                ),
                "distribution.top-level-library",
            ),
            (
                GOOD.replace("\"Lib.lvlib\"", "\"Missing.lvlib\""),
                "distribution.top-level-library",
            ),
            (
                GOOD.replace("\"Lib.lvlib\"", "\"../Out.lvlib\""),
                "distribution.top-level-library",
            ),
            (
                GOOD.replace("\"Lib.lvlib\"", "\"C:/Abs.lvlib\""),
                "distribution.top-level-library",
            ),
            (
                GOOD.replace("vendor = \"Vendor\"\n", ""),
                "distribution.vip.vendor",
            ),
            (
                GOOD.replace("display-name = \"Demo Lib\"\n", ""),
                "distribution.vip.display-name",
            ),
            (
                GOOD.replace("top-level-library", "kinds = [\"ppl\"]\ntop-level-library"),
                "distribution.kinds",
            ),
            (
                GOOD.replace(
                    "top-level-library",
                    "formats = [\"lvpkg\"]\ntop-level-library",
                ),
                "distribution.formats",
            ),
            (
                GOOD.replace("top-level-library", "colour = \"red\"\ntop-level-library"),
                "distribution.colour",
            ),
            (
                GOOD.replace("vendor =", "icon = \"i.bmp\"\nvendor ="),
                "distribution.vip.icon",
            ),
            (GOOD.replace("\"1.2.3\"", "\"1.2\""), "project.version"),
            (GOOD.replace("\"1.2.3\"", "\"1.a.3\""), "project.version"),
            (
                GOOD.replace("\"demo_lib\"", "\"@scope/lib\""),
                "distribution.vip.name",
            ),
            (
                GOOD.replace("\"@hexagon/lib-a\" = \"hexagon_lib_a\"", ""),
                "distribution.vip.dependency-names",
            ),
        ];
        let bad_dir = |d: &str| GOOD.replace("vendor =", &format!("install-dir = {d:?}\nvendor ="));
        let more = [
            (
                GOOD.replace("\"Lib.lvlib\"", r#""Sub\\Lib.lvlib""#),
                "distribution.top-level-library",
            ),
            (bad_dir("../x"), "distribution.vip.install-dir"),
            (bad_dir("/"), "distribution.vip.install-dir"),
            (bad_dir("C:/x"), "distribution.vip.install-dir"),
            (
                GOOD.replace("\"Demo Lib\"", r#""..\\..\\x""#),
                "distribution.vip.install-dir",
            ),
            (
                GOOD.replace(
                    "vendor =",
                    "name = \"x/../../pkg\"
vendor =",
                ),
                "distribution.vip.name",
            ),
            (
                GOOD.replace("\"Demo Lib\"", r#""Demo\"\n[Script VIs]""#),
                "distribution.vip.display-name",
            ),
            (GOOD.replace("\"hexagon_lib_a\"", "\"a,b\""), "dependencies"),
        ];
        for (i, (text, key)) in cases.into_iter().chain(more).enumerate() {
            let e = format!("{:#}", check(&text).unwrap_err());
            assert!(e.contains(key), "case {i}, {key}: {e}");
        }
        let unknown = GOOD.replace("vendor =", "icon = \"i.bmp\"\nvendor =");
        let e = format!("{:#}", check(&unknown).unwrap_err());
        assert!(e.contains("a later version adds it"), "{e}");
        let e = format!("{:#}", check("[project]\nname = \"x\"").unwrap_err());
        assert!(e.contains("[distribution]"), "{e}");
    }

    #[test]
    fn install_dir_overrides_and_four_parts_stay() {
        let d = check(
            &GOOD
                .replace(
                    "vendor =",
                    "install-dir = \"vi.lib/Other/Place/\"\nvendor =",
                )
                .replace("\"1.2.3\"", "\"2.1.0.5\""),
        )
        .unwrap();
        assert_eq!(d.install_dir, "vi.lib/Other/Place");
        assert_eq!(d.version, "2.1.0.5");
        let d = check(&GOOD.replace(
            "vendor =",
            r#"install-dir = 'vi.lib\Other\Place\'
vendor ="#,
        ))
        .unwrap();
        assert_eq!(d.install_dir, "vi.lib/Other/Place");
    }
}
