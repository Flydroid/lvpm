//! The browse list: every package in the catalog with its status relative to
//! the destination, narrowed by a search and a status filter.

use lvpm_core::index::{CACHE_SOURCE, Entry};
use lvpm_core::install::Manifest;
use lvpm_core::ops::query::{self, Catalog};
use lvpm_core::project::{name_key, same_name};
use lvpm_core::target::LvTarget;
use lvpm_core::version::Version;
use ratatui::widgets::TableState;

/// A package's state in the destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Installed,
    /// The version core would resolve is newer than the installed one.
    Update {
        available: Version,
    },
    NotInstalled,
    /// No version fits the destination's LabVIEW; the newest needs this year.
    NeedsLabview(u32),
}

impl Status {
    pub fn text(&self) -> String {
        match self {
            Status::Installed => "installed".into(),
            Status::Update { .. } => "update".into(),
            Status::NotInstalled => "not installed".into(),
            Status::NeedsLabview(year) => format!("needs LV {year}"),
        }
    }
}

/// The status rule. `versions` is one package's, newest first; the available
/// version is the one install would resolve for `target`.
pub fn status(
    versions: &[Entry],
    installed: Option<&Version>,
    target: Option<&LvTarget>,
) -> Status {
    let available = query::resolvable(versions, target).map(|e| &e.version);
    match (installed, available) {
        (Some(have), Some(newer)) if newer > have => Status::Update {
            available: newer.clone(),
        },
        (Some(_), _) => Status::Installed,
        (None, Some(_)) => Status::NotInstalled,
        (None, None) => Status::NeedsLabview(
            versions
                .first()
                .zip(target)
                .and_then(|(newest, t)| query::needs_newer_labview(newest, t))
                .unwrap_or_default(),
        ),
    }
}

/// One row of the list.
#[derive(Debug)]
pub struct Package {
    pub name: String,
    pub display_name: Option<String>,
    /// Newest first.
    pub versions: Vec<Entry>,
    pub installed: Option<Version>,
    /// The version the row offers: the one install would resolve, else the
    /// newest, which needs a newer LabVIEW.
    pub available: Option<Version>,
    pub status: Status,
}

impl Package {
    /// Where the offered version comes from, short enough for its column.
    pub fn source(&self) -> &str {
        let offered = self
            .versions
            .iter()
            .find(|e| Some(&e.version) == self.available.as_ref());
        offered.map_or("", |e| source_label(&e.source))
    }
}

/// A source, short: a feed by its host (`github` for GitHub's), else `local`
/// for a folder or `cache` for an imported archive.
pub fn source_label(source: &str) -> &str {
    let Some((_, rest)) = source.split_once("://") else {
        return if source == CACHE_SOURCE {
            "cache"
        } else {
            "local"
        };
    };
    let host = rest.split(['/', ':']).next().unwrap_or(rest);
    match host.ends_with("github.com") || host.ends_with("githubusercontent.com") {
        true => "github",
        false => host.strip_prefix("www.").unwrap_or(host),
    }
}

/// Which rows the filter keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Filter {
    #[default]
    All,
    Installed,
    Updates,
}

impl Filter {
    pub fn label(self) -> &'static str {
        match self {
            Filter::All => "all",
            Filter::Installed => "installed",
            Filter::Updates => "updates",
        }
    }

    fn next(self) -> Filter {
        match self {
            Filter::All => Filter::Installed,
            Filter::Installed => Filter::Updates,
            Filter::Updates => Filter::All,
        }
    }

    fn keeps(self, status: &Status) -> bool {
        match self {
            Filter::All => true,
            Filter::Installed => matches!(status, Status::Installed | Status::Update { .. }),
            Filter::Updates => matches!(status, Status::Update { .. }),
        }
    }
}

/// The loaded list and how it is narrowed.
#[derive(Debug, Default)]
pub struct Browse {
    pub packages: Vec<Package>,
    /// Every source asked, for the empty list to name.
    pub sources: Vec<String>,
    /// The sources that could not be read, with why.
    pub failed: Vec<(String, String)>,
    pub search: String,
    /// Keys go to the search box.
    pub searching: bool,
    pub filter: Filter,
    /// Indexes into `packages` of the rows shown.
    pub rows: Vec<usize>,
    pub table: TableState,
    /// How many rows the last draw showed: what PgUp and PgDn move by.
    pub page: usize,
    /// Why the last reload failed, while this older list is still shown.
    pub stale: Option<String>,
}

impl Browse {
    /// The list for `catalog`, whose entries are by name and newest first.
    pub fn new(catalog: Catalog, installed: &[Manifest], target: Option<&LvTarget>) -> Browse {
        let mut packages: Vec<Package> = Vec::new();
        for entry in catalog.entries {
            match packages.last_mut() {
                Some(last) if name_key(&last.name) == name_key(&entry.name) => {
                    last.versions.push(entry)
                }
                _ => packages.push(Package {
                    name: entry.name.clone(),
                    display_name: entry.display_name.clone(),
                    versions: vec![entry],
                    installed: None,
                    available: None,
                    status: Status::NotInstalled,
                }),
            }
        }
        for package in &mut packages {
            package.installed = installed
                .iter()
                .find(|m| same_name(&m.name, &package.name))
                .map(|m| Version::parse(&m.version));
            package.status = status(&package.versions, package.installed.as_ref(), target);
            package.available = query::resolvable(&package.versions, target)
                .or(package.versions.first())
                .map(|e| e.version.clone());
        }
        let mut browse = Browse {
            packages,
            sources: catalog.sources,
            failed: catalog.failed,
            page: 1,
            ..Browse::default()
        };
        browse.refilter();
        browse
    }

    pub fn selected(&self) -> Option<&Package> {
        let row = self.table.selected()?;
        self.rows.get(row).map(|&i| &self.packages[i])
    }

    /// Recompute the rows shown, keeping the selected package when it stays.
    fn refilter(&mut self) {
        let keep = self
            .table
            .selected()
            .and_then(|row| self.rows.get(row).copied());
        self.rows = (0..self.packages.len())
            .filter(|&i| {
                let p = &self.packages[i];
                p.versions.iter().any(|e| e.matches(&self.search)) && self.filter.keeps(&p.status)
            })
            .collect();
        let at = keep
            .and_then(|k| self.rows.iter().position(|&r| r == k))
            .unwrap_or(0);
        self.table.select((!self.rows.is_empty()).then_some(at));
    }

    /// Move the selection by `by` rows, stopping at either end.
    pub fn step(&mut self, by: isize) {
        let Some(last) = self.rows.len().checked_sub(1) else {
            return;
        };
        let at = self.table.selected().unwrap_or(0);
        self.table
            .select(Some(at.saturating_add_signed(by).min(last)));
    }

    pub fn page_size(&self) -> isize {
        self.page.max(1) as isize
    }

    pub fn next_filter(&mut self) {
        self.filter = self.filter.next();
        self.refilter();
    }

    pub fn type_char(&mut self, c: char) {
        self.search.push(c);
        self.refilter();
    }

    pub fn backspace(&mut self) {
        self.search.pop();
        self.refilter();
    }

    /// Narrow this freshly read list as `old` was, and keep its selected
    /// package selected while it is still listed.
    pub fn narrow_as(&mut self, old: &Browse) {
        self.search.clone_from(&old.search);
        self.searching = old.searching;
        self.filter = old.filter;
        self.page = old.page;
        self.refilter();
        let keep = old.selected().and_then(|package| {
            let mut rows = self.rows.iter();
            rows.position(|&i| same_name(&self.packages[i].name, &package.name))
        });
        if let Some(at) = keep {
            self.table.select(Some(at));
        }
    }

    /// Esc: the search is cleared and every package is listed again.
    pub fn clear_search(&mut self) {
        self.search.clear();
        self.searching = false;
        self.refilter();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn a_source_is_named_by_its_host_or_its_kind() {
        let label = source_label;
        assert_eq!(label("https://www.vipm.io/feeds/index.vipr"), "vipm.io");
        assert_eq!(label("http://host:8090/files/index.vipr"), "host");
        assert_eq!(
            label("https://raw.githubusercontent.com/o/r/main/i"),
            "github"
        );
        assert_eq!(label(r"C:\packages"), "local");
        assert_eq!(label(CACHE_SOURCE), "cache");
    }

    fn entry(version: &str, lv_min: f64) -> Entry {
        Entry {
            name: "alpha".into(),
            version: Version::parse(version),
            url: String::new(),
            md5: None,
            display_name: None,
            requires: Vec::new(),
            lv_min: Some(lv_min),
            source: String::new(),
        }
    }

    const LABVIEW_2025: LvTarget = LvTarget {
        version: 25.0,
        bitness: 64,
        path: PathBuf::new(),
    };

    fn of(versions: &[Entry], installed: Option<&str>) -> Status {
        status(
            versions,
            installed.map(Version::parse).as_ref(),
            Some(&LABVIEW_2025),
        )
    }

    #[test]
    fn a_newer_compatible_version_is_an_update() {
        let versions = [entry("2.0", 20.0), entry("1.0", 20.0)];
        assert_eq!(
            of(&versions, Some("1.0")),
            Status::Update {
                available: Version::parse("2.0")
            }
        );
        assert_eq!(of(&versions, Some("2.0")), Status::Installed);
        assert_eq!(of(&versions, None), Status::NotInstalled);
    }

    #[test]
    fn a_newer_version_the_labview_cannot_take_is_no_update() {
        let versions = [entry("2.0", 99.0), entry("1.0", 20.0)];
        assert_eq!(of(&versions, Some("1.0")), Status::Installed);
        assert_eq!(of(&versions, None), Status::NotInstalled);
    }

    #[test]
    fn a_package_with_no_version_that_fits_names_the_labview_it_needs() {
        let versions = [entry("1.0", 99.0)];
        assert_eq!(of(&versions, None), Status::NeedsLabview(2099));
        assert_eq!(
            status(&versions, None, None),
            Status::NotInstalled,
            "a destination without a LabVIEW gates nothing"
        );
    }

    #[test]
    fn an_installed_version_newer_than_any_listed_is_installed() {
        assert_eq!(of(&[entry("1.0", 20.0)], Some("3.0")), Status::Installed);
    }

    #[test]
    fn an_installed_package_no_listed_version_fits_is_still_installed() {
        assert_eq!(of(&[entry("1.0", 99.0)], Some("1.0")), Status::Installed);
    }

    #[test]
    fn a_gated_package_names_the_year_its_newest_version_needs() {
        let versions = [entry("2.0", 99.0), entry("1.0", 30.0)];
        assert_eq!(of(&versions, None), Status::NeedsLabview(2099));
    }

    #[test]
    fn each_status_reads_as_the_list_shows_it() {
        let texts = [
            Status::Installed,
            Status::Update {
                available: Version::parse("2.0"),
            },
            Status::NotInstalled,
            Status::NeedsLabview(2026),
        ]
        .map(|s| s.text());
        assert_eq!(
            texts,
            ["installed", "update", "not installed", "needs LV 2026"]
        );
    }

    /// One version each of `names`, the first with display name "Bravo Tools".
    fn browse(names: &[&str]) -> Browse {
        let entries = names
            .iter()
            .enumerate()
            .map(|(i, name)| Entry {
                name: name.to_string(),
                display_name: (i == 0).then(|| "Bravo Tools".to_string()),
                ..entry("1.0", 20.0)
            })
            .collect();
        let catalog = Catalog {
            entries,
            sources: Vec::new(),
            failed: Vec::new(),
        };
        Browse::new(catalog, &[], Some(&LABVIEW_2025))
    }

    fn shown(browse: &Browse) -> Vec<&str> {
        browse
            .rows
            .iter()
            .map(|&i| browse.packages[i].name.as_str())
            .collect()
    }

    #[test]
    fn a_row_offers_the_resolved_version_else_the_newest() {
        let offered = |entries: Vec<Entry>| {
            let catalog = Catalog {
                entries,
                sources: Vec::new(),
                failed: Vec::new(),
            };
            Browse::new(catalog, &[], Some(&LABVIEW_2025)).packages[0]
                .available
                .clone()
        };
        let newer_gated = vec![entry("2.0", 99.0), entry("1.0", 20.0)];
        assert_eq!(offered(newer_gated), Some(Version::parse("1.0")));
        assert_eq!(
            offered(vec![entry("2.0", 99.0)]),
            Some(Version::parse("2.0"))
        );
    }

    #[test]
    fn the_filter_cycles_all_installed_updates_and_back() {
        let mut list = browse(&["alpha", "beta", "gamma"]);
        list.packages[0].status = Status::Update {
            available: Version::parse("2.0"),
        };
        list.packages[1].status = Status::Installed;
        list.refilter();
        let mut seen = vec![shown(&list).join(",")];
        for _ in 0..3 {
            list.next_filter();
            seen.push(shown(&list).join(","));
        }
        assert_eq!(
            seen,
            [
                "alpha,beta,gamma",
                "alpha,beta",
                "alpha",
                "alpha,beta,gamma"
            ]
        );
    }

    #[test]
    fn steps_stop_at_either_end_and_pages_move_by_the_page() {
        let mut list = browse(&["a", "b", "c", "d", "e", "f", "g"]);
        list.step(-1);
        assert_eq!(list.table.selected(), Some(0));
        list.page = 5;
        list.step(list.page_size());
        assert_eq!(list.table.selected(), Some(5));
        list.step(list.page_size());
        assert_eq!(list.table.selected(), Some(6), "stops at the last row");
        list.step(-list.page_size());
        assert_eq!(list.table.selected(), Some(1));

        let mut empty = browse(&[]);
        empty.step(1);
        assert_eq!(empty.selected().map(|p| p.name.as_str()), None);
    }

    #[test]
    fn a_search_keeps_the_selected_package_while_it_still_matches() {
        let mut list = browse(&["alpha", "alps", "beta"]);
        list.step(1);
        list.type_char('a');
        list.type_char('l');
        assert_eq!(list.selected().map(|p| p.name.as_str()), Some("alps"));
        list.type_char('p');
        list.type_char('h');
        assert_eq!(list.selected().map(|p| p.name.as_str()), Some("alpha"));
        list.type_char('z');
        assert!(list.selected().is_none());
    }

    #[test]
    fn a_search_matches_a_display_name_in_any_case() {
        let mut list = browse(&["first", "second"]);
        for c in "BRAVO".chars() {
            list.type_char(c);
        }
        assert_eq!(shown(&list), ["first"]);
    }
}
