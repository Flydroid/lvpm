//! `[distribution]`: what `lvpm build` packages, and how it names it.
//!
//! ```toml
//! [distribution]
//! top-level-library = "HSE Logger.lvlib"  # relative to lvpm.toml, inside its folder
//! kinds        = ["source"]               # the only kind this version builds
//! vendor       = "Hampel Software Engineering"
//! display-name = "HSE Logger"
//! install-dir  = "vi.lib/Hampel Software Engineering/HSE Logger"  # default: vi.lib/<vendor>/<display-name>
//! ```
//!
//! The package name is `[project] name`.

use crate::lockfile::constraint_text;
use crate::lvpkg;
use crate::project::Project;
use anyhow::{Result, bail};
use std::path::{Component, Path, PathBuf};

/// A manifest checked for `lvpm build`: every key it needs, present and valid.
#[derive(Debug, PartialEq)]
pub struct Distribution {
    /// `top-level-library`, below the project folder.
    pub library: PathBuf,
    /// The package name: `[project] name`.
    pub name: String,
    /// `[project] version`, as written: three or four numbers.
    pub version: String,
    pub vendor: String,
    pub display_name: String,
    /// Where the members go, below the LabVIEW folder, with forward slashes.
    pub install_dir: String,
    /// `[project] labview`'s year.
    pub labview: u32,
    /// `[dependencies]` as the manifest writes them, in file order: name, constraint.
    pub dependencies: Vec<(String, String)>,
}

const DISTRIBUTION_KEYS: &[&str] = &[
    "top-level-library",
    "kinds",
    "vendor",
    "display-name",
    "install-dir",
];

const ADR_6: &str = "ADR 6, docs/adr/0006-lvpkg-only-vip-read-only.md";

/// `p`'s `[distribution]`, checked against the project folder `root`. Each
/// error names the full key it is about.
pub fn validate(p: &Project, root: &Path) -> Result<Distribution> {
    let Some(dist) = &p.distribution else {
        bail!(
            "lvpm.toml has no [distribution] table: lvpm build needs one naming the top-level-library"
        );
    };
    if dist.contains_key("formats") {
        bail!(
            "distribution.formats was removed ({ADR_6}): lvpm build always writes a .lvpkg. Delete the line."
        );
    }
    if let Some(vip) = dist.get("vip") {
        let found = match vip {
            toml::Value::Table(t) if t.is_empty() => "(empty table)".to_string(),
            toml::Value::Table(t) => t.keys().map(String::as_str).collect::<Vec<_>>().join(", "),
            _ => "(not a table)".to_string(),
        };
        bail!(
            "[distribution.vip] was removed ({ADR_6}). Move vendor, display-name and install-dir into \
             [distribution]; the package name is [project] name; icon and dependency-names are gone. \
             Found: {found}"
        );
    }
    unknown_keys("distribution", dist, DISTRIBUTION_KEYS)?;
    only(dist, "kinds", "source")?;

    let library = top_level_library(dist, root)?;
    let vendor = required(dist, "distribution", "vendor")?;
    let display_name = required(dist, "distribution", "display-name")?;
    // The default goes through the same check: a vendor of `..\x` must not leave vi.lib.
    let install_dir = match string(dist, "distribution", "install-dir")? {
        Some(d) => d,
        None => format!("vi.lib/{vendor}/{display_name}"),
    };
    // A trailing slash is harmless; anything else that leaves the LabVIEW folder is not.
    let install_dir = install_dir.trim_end_matches('/').to_string();
    lvpkg::check_install_dir(&install_dir)?;

    let name = match &p.name {
        Some(n) if n.starts_with('@') => bail!(
            "project.name = {n:?}: scoped package names are not supported yet; use an unscoped name"
        ),
        Some(n) => n.clone(),
        None => bail!("project.name: lvpm build needs the package name"),
    };
    lvpkg::check_name(&name)?;
    let Some(version) = &p.version else {
        bail!("project.version: lvpm build needs the package version");
    };
    check_version(version)?;
    let Some(labview) = &p.labview else {
        bail!("project.labview: lvpm build needs the oldest LabVIEW, like labview = \">=2016\"");
    };
    let labview = crate::project::labview_year(labview)?;

    for (key, value) in [
        ("distribution.vendor", &vendor),
        ("distribution.display-name", &display_name),
        ("distribution.install-dir", &install_dir),
    ] {
        spec_safe(key, value)?;
    }
    for (dep, c) in &p.dependencies {
        let item = lvpkg::requires_item(dep, c);
        spec_safe("dependencies", &item)?;
        if item.contains(',') {
            bail!(
                "dependencies: {item:?} holds a ',', which would split the package's Requires list"
            );
        }
    }
    let dependencies = p
        .dependencies
        .iter()
        .map(|(dep, c)| (dep.clone(), constraint_text(c)))
        .collect();

    Ok(Distribution {
        library,
        name,
        version: version.clone(),
        vendor,
        display_name,
        install_dir,
        labview,
        dependencies,
    })
}

impl Distribution {
    /// The package file's name, `<name>-<version>.lvpkg`.
    pub fn file_name(&self) -> String {
        format!("{}-{}.lvpkg", self.name, self.version)
    }

    /// What goes into the package's `package.toml`.
    pub fn meta(&self) -> lvpkg::Meta {
        lvpkg::Meta {
            name: self.name.clone(),
            version: self.version.clone(),
            labview: format!(">={}", self.labview),
            vendor: self.vendor.clone(),
            display_name: self.display_name.clone(),
            install_dir: self.install_dir.clone(),
            dependencies: self.dependencies.clone(),
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

/// Three or four numeric parts.
fn check_version(v: &str) -> Result<()> {
    let parts: Vec<&str> = v.split('.').collect();
    let numeric = parts
        .iter()
        .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    match parts.len() {
        3 | 4 if numeric => Ok(()),
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
vendor = "Vendor"
display-name = "Demo Lib"
"#;

    fn check(text: &str) -> Result<Distribution> {
        validate(&crate::project::parse(text).unwrap(), &root())
    }

    fn error(text: &str) -> String {
        format!("{:#}", check(text).unwrap_err())
    }

    #[test]
    fn a_complete_manifest_gives_names_version_install_dir_and_dependencies_as_written() {
        let d = check(GOOD).unwrap();
        assert_eq!(d.file_name(), "demo_lib-1.2.3.lvpkg");
        assert_eq!(d.install_dir, "vi.lib/Vendor/Demo Lib");
        let written = [
            ("a", "1.0"),
            ("b", ">=2.0"),
            ("c", "*"),
            ("@hexagon/lib-a", "3.0"),
        ];
        assert_eq!(
            d.dependencies,
            written.map(|(n, c)| (n.to_string(), c.to_string()))
        );
        assert_eq!(d.library, Path::new("Lib.lvlib"));
    }

    #[test]
    fn meta_is_what_package_toml_holds() {
        let m = check(GOOD).unwrap().meta();
        assert_eq!(
            (m.name.as_str(), m.version.as_str(), m.labview.as_str()),
            ("demo_lib", "1.2.3", ">=2016")
        );
        assert_eq!(
            (m.vendor.as_str(), m.display_name.as_str()),
            ("Vendor", "Demo Lib")
        );
        assert_eq!(m.install_dir, "vi.lib/Vendor/Demo Lib");
        assert_eq!(m.dependencies.len(), 4);
    }

    #[test]
    fn each_problem_names_its_key() {
        let cases = [
            (
                GOOD.replace("top-level-library = \"Lib.lvlib\"\n", ""),
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
                GOOD.replace("\"Lib.lvlib\"", r#""Sub\\Lib.lvlib""#),
                "distribution.top-level-library",
            ),
            (
                GOOD.replace("vendor = \"Vendor\"\n", ""),
                "distribution.vendor",
            ),
            (
                GOOD.replace("display-name = \"Demo Lib\"\n", ""),
                "distribution.display-name",
            ),
            (
                GOOD.replace("top-level-library", "kinds = [\"ppl\"]\ntop-level-library"),
                "distribution.kinds",
            ),
            (
                GOOD.replace("top-level-library", "colour = \"red\"\ntop-level-library"),
                "distribution.colour",
            ),
            (GOOD.replace("\"1.2.3\"", "\"1.2\""), "project.version"),
            (GOOD.replace("\"1.2.3\"", "\"1.a.3\""), "project.version"),
            (
                GOOD.replace("\"demo_lib\"", "\"x/../../pkg\""),
                "project.name",
            ),
            (
                GOOD.replace("\"Demo Lib\"", r#""Demo\"\n[Script VIs]""#),
                "distribution.display-name",
            ),
            (GOOD.replace("3.0", "3,0"), "dependencies"),
        ];
        for (i, (text, key)) in cases.into_iter().enumerate() {
            let e = error(&text);
            assert!(e.contains(key), "case {i}, {key}: {e}");
        }
        let e = error(&GOOD.replace("vendor =", "icon = \"i.bmp\"\nvendor ="));
        assert!(
            e.contains("distribution.icon") && e.contains("a later version adds it"),
            "{e}"
        );
        assert!(error("[project]\nname = \"x\"").contains("[distribution]"));
    }

    #[test]
    fn a_bad_install_dir_names_its_key() {
        let with = |d: &str| GOOD.replace("vendor =", &format!("install-dir = {d:?}\nvendor ="));
        for dir in [
            "../x", "a/../x", "/", "/x", "C:/x", r"a\b", r"..\..\x", ".", "a/./b", "a//b",
            "a/.. /x",
        ] {
            let e = error(&with(dir));
            assert!(e.contains("distribution.install-dir"), "{dir}: {e}");
        }
        // the default goes through the same check
        let e = error(&GOOD.replace("\"Demo Lib\"", r#""..\\..\\x""#));
        assert!(e.contains("distribution.install-dir"), "{e}");
    }

    #[test]
    fn install_dir_overrides_and_the_version_stays_as_written() {
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
    }

    #[test]
    fn formats_is_removed_whatever_it_holds() {
        for value in ["[\"vip\"]", "[\"lvpkg\"]", "\"vip\""] {
            let e = error(&GOOD.replace("vendor =", &format!("formats = {value}\nvendor =")));
            assert!(
                e.contains(
                    "distribution.formats was removed (ADR 6, docs/adr/0006-lvpkg-only-vip-read-only.md): \
                     lvpm build always writes a .lvpkg. Delete the line."
                ),
                "{value}: {e}"
            );
        }
    }

    #[test]
    fn distribution_vip_is_removed_and_lists_every_key_in_file_order() {
        let removed = "[distribution.vip] was removed (ADR 6, docs/adr/0006-lvpkg-only-vip-read-only.md). \
                       Move vendor, display-name and install-dir into [distribution]; the package name \
                       is [project] name; icon and dependency-names are gone. Found: ";
        for (vip, found) in [
            ("vendor = \"V\"\n", "vendor"),
            ("", "(empty table)"),
            (
                "[distribution.vip.dependency-names]\na = \"b\"\n",
                "dependency-names",
            ),
            (
                "vendor = \"V\"\ndisplay-name = \"D\"\nname = \"n\"\nicon = \"i.bmp\"\n",
                "vendor, display-name, name, icon",
            ),
        ] {
            let e = error(&format!("{GOOD}\n[distribution.vip]\n{vip}"));
            assert!(e.contains(&format!("{removed}{found}")), "{found}: {e}");
        }
        let e = error(&GOOD.replace("vendor =", "vip = \"x\"\nvendor ="));
        assert!(e.contains(&format!("{removed}(not a table)")), "{e}");
    }

    #[test]
    fn formats_is_reported_before_vip_and_both_before_unknown_keys() {
        let both = format!("{GOOD}\nformats = [\"vip\"]\ncolour = \"red\"\n[distribution.vip]\n");
        assert!(error(&both).contains("distribution.formats was removed"));
        let vip = format!("{GOOD}\n[distribution.vip]\n");
        let vip = vip.replace("vendor =", "colour = \"red\"\nvendor =");
        assert!(error(&vip).contains("[distribution.vip] was removed"));
    }

    #[test]
    fn a_scoped_project_name_is_refused() {
        let e = error(&GOOD.replace("\"demo_lib\"", "\"@scope/lib\""));
        assert!(
            e.contains(
                "project.name = \"@scope/lib\": scoped package names are not supported yet; \
                 use an unscoped name"
            ),
            "{e}"
        );
        // an '@' inside a name is just a bad character, not a scope
        let e = error(&GOOD.replace("\"demo_lib\"", "\"lib@2\""));
        assert!(e.contains("a package name takes letters"), "{e}");
    }
}
