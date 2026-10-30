//! The `spec` manifest inside a package.
//!
//! The format is the OpenG Package Tools' "Package Spec File"
//! (<https://ogpm.sourceforge.net/design/Package%20Spec%20File.htm>; see
//! docs/ogpm-model.md). A package is a plain zip containing `spec` (this
//! file), an icon, and one `File Group N/` payload tree per file group. `spec`
//! is INI, values optionally quoted; the parts that matter for installing are
//! `[File Group N]` and `[Script VIs]`.

use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize)]
pub struct FileGroup {
    pub index: usize,
    pub target_dir: String,
    pub replace_mode: String,
    pub files: Vec<String>,
    /// Where the group's files sit in the archive: `File Group N/` in a `.vip`,
    /// the archive root (empty) in a `.lvpkg`.
    #[serde(skip)]
    pub entry_prefix: String,
}

impl FileGroup {
    /// The zip entry that holds `file` of this group.
    pub fn entry(&self, file: &str) -> String {
        format!("{}{}", self.entry_prefix, file.replace('\\', "/"))
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Spec {
    pub name: String,
    pub version: String,
    pub display_name: Option<String>,
    pub file_groups: Vec<FileGroup>,
    /// Non-empty `[Script VIs]` hooks, e.g. ("PostInstall", "install.vi").
    /// About a quarter of published packages have at least one.
    pub script_vis: Vec<(String, String)>,
    /// `[Dependencies] Requires`, verbatim. Local package folders use this
    /// when resolving dependencies from a package's own spec.
    pub requires: Option<String>,
    /// `[Platform] Exclusive_LabVIEW_Version`, verbatim — e.g. `LabVIEW>=25.3`.
    /// Some packages write it bare, as `>=8.6`.
    pub lv_gate: Option<String>,
    /// The `[Description]` section, for people to read.
    pub description: Description,
}

/// The `[Description]` section, for people to read. A field the spec leaves
/// out or leaves empty is `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Description {
    pub description: Option<String>,
    pub summary: Option<String>,
    pub vendor: Option<String>,
    pub packager: Option<String>,
    pub license: Option<String>,
    pub url: Option<String>,
    pub release_notes: Option<String>,
}

const HOOKS: &[&str] = &[
    "PreInstall",
    "PostInstall",
    "PreUninstall",
    "PostUninstall",
    "Verify",
    "PreBuild",
    "PostBuild",
];

pub fn parse(text: &str) -> Result<Spec> {
    // section -> key -> value
    let mut sections: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut current = String::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            current = name.to_string();
            sections.entry(current.clone()).or_default();
        } else if let Some((k, v)) = line.split_once('=') {
            sections
                .entry(current.clone())
                .or_default()
                .insert(k.trim().to_string(), v.trim().trim_matches('"').to_string());
        }
    }

    // Two dialects live in the public corpus: the OGPT design (and `.ogp`
    // files) use `[Package Name]`; VIPM's `.vip` renamed it `[Package]`.
    let pkg = sections
        .get("Package")
        .or_else(|| sections.get("Package Name"))
        .context("spec has neither a [Package] nor a [Package Name] section")?;

    let name = pkg.get("Name").context("spec has no package Name")?.clone();

    // OGPT names a package `<name>-<version>-<release>`: Version=1.1 plus
    // Release=1 is "1.1-1", which is what the package directory calls it.
    // `.vip` dropped Release and carries a four-part Version instead.
    let version = match (pkg.get("Version"), pkg.get("Release")) {
        (Some(v), Some(r)) if !r.trim().is_empty() => format!("{}-{}", v.trim(), r.trim()),
        (Some(v), _) => v.trim().to_string(),
        (None, _) => String::new(),
    };

    let display_name = pkg.get("Display Name").cloned().filter(|s| !s.is_empty());

    let mut file_groups = Vec::new();
    for i in 0.. {
        let Some(g) = sections.get(&format!("File Group {i}")) else {
            break;
        };
        // The `File <n>=` keys there are, below Num Files when it is given.
        // Counting up to Num Files instead would let a spec spin on a huge one.
        let num_files = g.get("Num Files").and_then(|n| n.parse::<usize>().ok());
        let mut numbered: Vec<(usize, String)> = g
            .iter()
            .filter_map(|(k, val)| {
                let n = k.strip_prefix("File ")?.parse::<usize>().ok()?;
                (num_files.is_none_or(|max| n < max)).then(|| (n, val.clone()))
            })
            .collect();
        numbered.sort_by_key(|(n, _)| *n);
        let files = numbered.into_iter().map(|(_, val)| val).collect();
        file_groups.push(FileGroup {
            index: i,
            target_dir: g.get("Target Dir").cloned().unwrap_or_default(),
            replace_mode: g
                .get("Replace Mode")
                .cloned()
                .unwrap_or_else(|| "Always".into()),
            files,
            entry_prefix: format!("File Group {i}/"),
        });
    }

    let script_vis = sections
        .get("Script VIs")
        .map(|s| {
            HOOKS
                .iter()
                .filter_map(|h| {
                    let v = s.get(*h)?;
                    (!v.trim().is_empty()).then(|| (h.to_string(), v.clone()))
                })
                .collect()
        })
        .unwrap_or_default();

    let non_empty = |s: &HashMap<String, String>, k: &str| {
        s.get(k)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    let requires = sections
        .get("Dependencies")
        .and_then(|s| non_empty(s, "Requires"));
    let lv_gate = sections
        .get("Platform")
        .and_then(|s| non_empty(s, "Exclusive_LabVIEW_Version"));
    let field = |k: &str| sections.get("Description").and_then(|s| non_empty(s, k));
    let description = Description {
        description: field("Description"),
        summary: field("Summary"),
        vendor: field("Vendor"),
        packager: field("Packager"),
        license: field("License"),
        url: field("URL"),
        release_notes: field("Release Notes"),
    };

    Ok(Spec {
        name,
        version,
        display_name,
        file_groups,
        script_vis,
        requires,
        lv_gate,
        description,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Trimmed from a real package: nfg_lib_tree_control_api-1.0.2.3.vip
    const SAMPLE: &str = r#"
[Package]
Name="nfg_lib_tree_control_api"
Version="1.0.2.3"
Display Name="Tree Control API"

[Script VIs]
PreInstall=""
PostInstall=""
PreUninstall=""

[Files]
Num File Groups="2"

[File Group 0]
Target Dir="<application>"
Replace Mode="Always"
Num Files=2
File 0="vi.lib/addons/_LavaCR/Tree Control API/a.mnu"
File 1="vi.lib/addons/_LavaCR/Tree Control API/_lava.llb"

[File Group 1]
Target Dir="<menus>/Categories"
Replace Mode="If Newer"
Num Files=1
File 0="functions_nfg.mnu"
"#;

    #[test]
    fn parses_file_groups_and_finds_no_hooks() {
        let s = parse(SAMPLE).unwrap();
        assert_eq!(s.name, "nfg_lib_tree_control_api");
        assert_eq!(s.version, "1.0.2.3");
        assert_eq!(s.file_groups.len(), 2);
        assert_eq!(s.file_groups[0].target_dir, "<application>");
        assert_eq!(s.file_groups[0].files.len(), 2);
        assert_eq!(s.file_groups[1].replace_mode, "If Newer");
        assert_eq!(s.file_groups[1].entry(r"a\b.vi"), "File Group 1/a/b.vi");
        assert!(s.script_vis.is_empty(), "empty hooks must not be reported");
    }

    #[test]
    fn num_files_bounds_the_files_read_and_a_huge_one_does_not_spin() {
        let group = |num: &str| {
            format!("[Package]\nName=a\nVersion=1.0\n[File Group 0]\n{num}File 0=x\nFile 1=y\n")
        };
        let files = |num: &str| parse(&group(num)).unwrap().file_groups[0].files.clone();
        assert_eq!(files("Num Files=1\n"), ["x"]);
        assert_eq!(files(""), ["x", "y"]);
        assert_eq!(files(&format!("Num Files={}\n", usize::MAX)), ["x", "y"]);
    }

    // OpenG-era `.ogp`: different section name, unquoted values, and the
    // version split across Version/Release. Trimmed from
    // jki_rsc_toolkits_palette-1.1-1.ogp.
    const LEGACY_OGP: &str = r#"
[Package Name]
Name=jki_rsc_toolkits_palette
Version=1.1
Release=1

[Script VIs]
Source Dir="../../JKI Toolkits Palette"
PostInstall=""

[File Group 0]
Target Dir="<menus>/Categories"
Replace Mode="Always"
Num Files=1
File 0="JKI Toolkits.mnu"
"#;

    #[test]
    fn parses_the_legacy_ogp_dialect() {
        let s = parse(LEGACY_OGP).unwrap();
        assert_eq!(s.name, "jki_rsc_toolkits_palette");
        // must match the id the index uses, so uninstall can find it again
        assert_eq!(s.version, "1.1-1");
        assert_eq!(s.file_groups.len(), 1);
        assert!(s.script_vis.is_empty());
    }

    #[test]
    fn reads_every_description_field() {
        let text = format!(
            "{SAMPLE}\n[Description]\nDescription=\"Edits trees.\"\nSummary=\"Tree API\"\n\
             Vendor=\"NFG\"\nPackager=\"Ann\"\nLicense=\"BSD\"\nURL=\"http://nfg.example\"\n\
             Release Notes=\"First.\"\n"
        );
        let d = parse(&text).unwrap().description;
        assert_eq!(
            d,
            Description {
                description: Some("Edits trees.".into()),
                summary: Some("Tree API".into()),
                vendor: Some("NFG".into()),
                packager: Some("Ann".into()),
                license: Some("BSD".into()),
                url: Some("http://nfg.example".into()),
                release_notes: Some("First.".into()),
            }
        );
    }

    #[test]
    fn a_missing_section_and_an_empty_value_are_both_absent() {
        assert_eq!(parse(SAMPLE).unwrap().description, Description::default());
        let text = format!("{SAMPLE}\n[Description]\nSummary=\"\"\nVendor=\"NFG\"\n");
        let d = parse(&text).unwrap().description;
        assert_eq!(d.summary, None);
        assert_eq!(d.vendor.as_deref(), Some("NFG"));
    }

    #[test]
    fn reports_non_empty_hooks() {
        let text = SAMPLE.replace(r#"PostInstall="""#, r#"PostInstall="setup.vi""#);
        let s = parse(&text).unwrap();
        assert_eq!(
            s.script_vis,
            vec![("PostInstall".into(), "setup.vi".into())]
        );
    }
}
