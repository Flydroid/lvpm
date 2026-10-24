//! The wording of query results: what `targets`, `search`, `list` and
//! `cache add` print for the data core returns. Each function returns the whole
//! text, line ends included, for the caller to write to stdout.

use lvpm_core::index::{Entry, Import};
use lvpm_core::install::Manifest;
use lvpm_core::target::LvTarget;

/// How many search hits are shown; the rest are counted.
const SEARCH_ROWS_SHOWN: usize = 40;

pub fn targets(targets: &[LvTarget]) -> String {
    if targets.is_empty() {
        return "no LabVIEW installations detected\n".to_string();
    }
    targets
        .iter()
        .map(|t| format!("{:<34} {}\n", t.label(), t.path.display()))
        .collect()
}

pub fn search(query: &str, hits: &[Entry]) -> String {
    if hits.is_empty() {
        return format!("no packages matching {query:?}\n");
    }
    let mut text: String = hits
        .iter()
        .take(SEARCH_ROWS_SHOWN)
        .map(|e| {
            format!(
                "{:<52} {:<14} {}\n",
                e.name,
                e.version.to_string(),
                e.display_name.as_deref().unwrap_or("")
            )
        })
        .collect();
    if hits.len() > SEARCH_ROWS_SHOWN {
        text.push_str(&format!(
            "... and {} more\n",
            hits.len() - SEARCH_ROWS_SHOWN
        ));
    }
    text
}

pub fn list(installed: &[Manifest]) -> String {
    if installed.is_empty() {
        return "lvpm has installed nothing into this target\n".to_string();
    }
    installed
        .iter()
        .map(|m| {
            format!(
                "{:<52} {:<14} {:>5} files  {:<12}{}\n",
                m.name,
                m.version,
                m.files.len(),
                if m.relinked {
                    "relinked"
                } else {
                    "NOT relinked"
                },
                if m.skipped_hooks.is_empty() {
                    String::new()
                } else {
                    format!(
                        "  (hooks skipped: {})",
                        super::hooks::hook_names(&m.skipped_hooks)
                    )
                }
            )
        })
        .collect()
}

pub fn cache_add(imports: &[Import]) -> String {
    let mut text: String = imports
        .iter()
        .map(|import| {
            format!(
                "{} {} sha256:{}\n",
                if import.reused { "cached" } else { "imported" },
                import.path.display(),
                import.sha256
            )
        })
        .collect();
    text.push_str(&format!("{} package(s) processed\n", imports.len()));
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use lvpm_core::version::Version;
    use std::path::PathBuf;

    fn entry(name: &str, display_name: Option<&str>) -> Entry {
        Entry {
            name: name.into(),
            version: Version::parse("1.0"),
            url: String::new(),
            md5: None,
            display_name: display_name.map(str::to_owned),
            requires: Vec::new(),
            lv_min: None,
        }
    }

    fn manifest(name: &str, relinked: bool, skipped_hooks: &[&str]) -> Manifest {
        Manifest {
            name: name.into(),
            version: "1.0".into(),
            display_name: None,
            installed_at: 0,
            target: String::new(),
            files: vec!["a".into(), "b".into()],
            skipped_hooks: skipped_hooks.iter().map(|h| h.to_string()).collect(),
            relinked,
            relink_folders: Vec::new(),
            post_install_vi: None,
            pre_install_vi: None,
            pre_uninstall_vi: None,
            post_uninstall_vi: None,
            skipped_groups: Vec::new(),
            archive_sha256: None,
        }
    }

    #[test]
    fn no_installations_and_no_hits_and_nothing_installed_are_each_said_so() {
        assert_eq!(targets(&[]), "no LabVIEW installations detected\n");
        assert_eq!(search("zzz", &[]), "no packages matching \"zzz\"\n");
        assert_eq!(list(&[]), "lvpm has installed nothing into this target\n");
    }

    #[test]
    fn a_search_shows_forty_hits_and_counts_the_rest() {
        let hits: Vec<Entry> = (0..43)
            .map(|n| entry(&format!("pkg{n:02}"), Some("A package")))
            .collect();
        let text = search("pkg", &hits);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 41);
        assert!(lines[0].starts_with("pkg00 "), "{}", lines[0]);
        assert!(
            lines[0].ends_with(" 1.0            A package"),
            "{}",
            lines[0]
        );
        assert_eq!(lines[39].split(' ').next(), Some("pkg39"));
        assert_eq!(lines[40], "... and 3 more");

        let exactly_forty = search("pkg", &hits[..40]);
        assert_eq!(exactly_forty.lines().count(), 40);
    }

    #[test]
    fn a_listed_package_says_whether_it_is_relinked_and_which_hooks_were_skipped() {
        let text = list(&[
            manifest("alpha", true, &[]),
            manifest(
                "beta",
                false,
                &["PostInstall=C:\\build\\Post.vi", "PreInstall=x"],
            ),
        ]);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(
            lines[0].ends_with("    2 files  relinked    "),
            "{}",
            lines[0]
        );
        assert!(
            lines[1].ends_with("NOT relinked  (hooks skipped: PostInstall, PreInstall)"),
            "{}",
            lines[1]
        );
    }

    #[test]
    fn imports_are_listed_then_counted() {
        let imports = [
            Import {
                path: PathBuf::from("a.vip"),
                sha256: "ab".into(),
                reused: false,
            },
            Import {
                path: PathBuf::from("b.vip"),
                sha256: "cd".into(),
                reused: true,
            },
        ];
        assert_eq!(
            cache_add(&imports),
            "imported a.vip sha256:ab\ncached b.vip sha256:cd\n2 package(s) processed\n"
        );
        assert_eq!(cache_add(&[]), "0 package(s) processed\n");
    }
}
