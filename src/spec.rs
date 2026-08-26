//! The `spec` manifest inside a `.vip`.
//!
//! A `.vip` is a plain zip containing `spec` (this file), `icon.bmp`, and one
//! `File Group N/` payload tree per file group. `spec` is INI with quoted
//! values; the parts that matter for installing are `[File Group N]` and
//! `[Script VIs]`.

use anyhow::{Context, Result};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct FileGroup {
    pub index: usize,
    pub target_dir: String,
    pub replace_mode: String,
    pub files: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Spec {
    pub name: String,
    pub version: String,
    pub display_name: Option<String>,
    pub file_groups: Vec<FileGroup>,
    /// Non-empty `[Script VIs]` hooks, e.g. ("PostInstall", "install.vi").
    /// About a quarter of published packages have at least one.
    pub script_vis: Vec<(String, String)>,
    /// `[Dependencies] Requires`, verbatim. Same syntax the indexes use for
    /// `Dependencies.Requires`, which is what lets a package on disk be
    /// resolved exactly like one from a feed.
    pub requires: Option<String>,
    /// `[Platform] Exclusive_LabVIEW_Version`, verbatim — e.g. `LabVIEW>=25.3`.
    /// Some packages write it bare, as `>=8.6`.
    pub lv_gate: Option<String>,
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

    // Two dialects live in the public corpus: modern `.vip` files use
    // `[Package]`, while OpenG-era `.ogp` files use `[Package Name]`.
    let pkg = sections
        .get("Package")
        .or_else(|| sections.get("Package Name"))
        .context("spec has neither a [Package] nor a [Package Name] section")?;

    let name = pkg.get("Name").context("spec has no package Name")?.clone();

    // `.ogp` splits the version: Version=1.1 plus Release=1 means "1.1-1",
    // which is what the index calls it.
    let version = match (pkg.get("Version"), pkg.get("Release")) {
        (Some(v), Some(r)) if !r.trim().is_empty() => format!("{}-{}", v.trim(), r.trim()),
        (Some(v), _) => v.trim().to_string(),
        (None, _) => String::new(),
    };

    let display_name = pkg.get("Display Name").cloned().filter(|s| !s.is_empty());

    let mut file_groups = Vec::new();
    for i in 0.. {
        let Some(g) = sections.get(&format!("File Group {i}")) else { break };
        let files = match g.get("Num Files").and_then(|n| n.parse::<usize>().ok()) {
            Some(n) => (0..n)
                .filter_map(|j| g.get(&format!("File {j}")).cloned())
                .collect(),
            // Fall back to scanning `File <n>=` keys when Num Files is absent.
            None => {
                let mut v: Vec<(usize, String)> = g
                    .iter()
                    .filter_map(|(k, val)| {
                        k.strip_prefix("File ")?.parse::<usize>().ok().map(|n| (n, val.clone()))
                    })
                    .collect();
                v.sort_by_key(|(n, _)| *n);
                v.into_iter().map(|(_, val)| val).collect()
            }
        };
        file_groups.push(FileGroup {
            index: i,
            target_dir: g.get("Target Dir").cloned().unwrap_or_default(),
            replace_mode: g.get("Replace Mode").cloned().unwrap_or_else(|| "Always".into()),
            files,
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
        s.get(k).map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
    };
    let requires = sections.get("Dependencies").and_then(|s| non_empty(s, "Requires"));
    let lv_gate =
        sections.get("Platform").and_then(|s| non_empty(s, "Exclusive_LabVIEW_Version"));

    Ok(Spec { name, version, display_name, file_groups, script_vis, requires, lv_gate })
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
        assert!(s.script_vis.is_empty(), "empty hooks must not be reported");
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
    fn reports_non_empty_hooks() {
        let text = SAMPLE.replace(r#"PostInstall="""#, r#"PostInstall="setup.vi""#);
        let s = parse(&text).unwrap();
        assert_eq!(s.script_vis, vec![("PostInstall".into(), "setup.vi".into())]);
    }
}
