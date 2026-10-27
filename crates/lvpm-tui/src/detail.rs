//! The detail view: one package's versions on the left, the picked version's
//! details on the right. Details are read on demand, one version at a time.

use crate::browse::Package;
use lvpm_core::index::Entry;
use lvpm_core::ops::query::PackageInfo;
use lvpm_core::version::Version;
use ratatui::widgets::TableState;

/// Which pane Up and Down move.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Focus {
    #[default]
    Versions,
    Fields,
}

#[derive(Debug)]
pub struct Detail {
    pub name: String,
    pub display_name: Option<String>,
    /// Newest first; never empty, as a listed package has a version.
    pub versions: Vec<Entry>,
    pub installed: Option<Version>,
    pub table: TableState,
    pub focus: Focus,
    /// How many lines the fields are scrolled by.
    pub scroll: u16,
    /// How many lines of fields the last draw showed: what PgUp and PgDn
    /// scroll by.
    pub page: u16,
}

impl Detail {
    /// The view of `package`, on its newest version.
    pub fn new(package: &Package) -> Detail {
        Detail {
            name: package.name.clone(),
            display_name: package.display_name.clone(),
            versions: package.versions.clone(),
            installed: package.installed.clone(),
            table: TableState::default().with_selected(0),
            focus: Focus::default(),
            scroll: 0,
            page: 1,
        }
    }

    /// The view of `package` as a reload lists it, on the version picked
    /// here if it is still listed.
    pub fn reopened(&self, package: &Package) -> Detail {
        let mut detail = Detail::new(package);
        let picked = &self.entry().version;
        let at = detail.versions.iter().position(|e| &e.version == picked);
        detail.table.select(Some(at.unwrap_or(0)));
        detail.focus = self.focus;
        detail
    }

    /// The version picked.
    pub fn entry(&self) -> &Entry {
        let at = self.table.selected().unwrap_or(0);
        &self.versions[at.min(self.versions.len() - 1)]
    }

    /// The name and version the picked version's details are kept under.
    pub fn key(&self) -> (String, String) {
        (self.name.clone(), self.entry().version.to_string())
    }

    /// Pick the version `by` rows away, stopping at either end; its fields
    /// show from the top.
    pub fn step(&mut self, by: isize) {
        let at = self.table.selected().unwrap_or(0);
        let last = self.versions.len() - 1;
        self.table
            .select(Some(at.saturating_add_signed(by).min(last)));
        self.scroll = 0;
    }

    /// Scroll the fields by `by` lines; the draw stops it at the last line.
    pub fn scroll_by(&mut self, by: isize) {
        let to = (self.scroll as isize).saturating_add(by).max(0);
        self.scroll = u16::try_from(to).unwrap_or(u16::MAX);
    }
}

/// The fields pane's text for `entry`: what the listing says, then, once
/// read, the details, else the loading line or core's error.
pub fn fields(detail: &Detail, info: Option<&Result<PackageInfo, String>>) -> String {
    let entry = detail.entry();
    let mut text = String::new();
    if let Some(display_name) = &detail.display_name {
        text.push_str(&format!("{display_name}\n\n"));
    }
    let mut field = |label: &str, value: &str| {
        text.push_str(&format!("{label:<14}{value}\n"));
    };
    field("name", &entry.name);
    field("source", &entry.source);
    if let Some(installed) = &detail.installed {
        field("installed", &installed.to_string());
    }
    if !entry.requires.is_empty() {
        field("requires", &requirements(entry));
    }
    if let Some(lv_min) = entry.lv_min {
        field("LabVIEW", &format!("{} or newer", labview_year(lv_min)));
    }
    text.push('\n');
    let what = format!("{} {}", entry.name, entry.version);
    match info {
        None => text.push_str(&format!("Reading the details of {what}…")),
        Some(Err(error)) => text.push_str(&format!(
            "The details of {what} could not be read:\n{error}"
        )),
        Some(Ok(info)) => text.push_str(&details(info)),
    }
    text
}

/// The spec's fields as `lvpm info` names them, then the file groups and the
/// release notes.
fn details(info: &PackageInfo) -> String {
    let d = &info.spec.description;
    let fields = [
        ("summary", &d.summary),
        ("description", &d.description),
        ("vendor", &d.vendor),
        ("packager", &d.packager),
        ("license", &d.license),
        ("url", &d.url),
    ];
    let mut text = String::new();
    for (label, value) in fields {
        if let Some(value) = value {
            text.push_str(&format!("{label:<14}{value}\n"));
        }
    }
    for group in &info.spec.file_groups {
        let files = group.files.len();
        text.push_str(&format!("{:<14}{files} to {}\n", "files", group.target_dir));
    }
    if let Some(notes) = &d.release_notes {
        text.push_str(&format!("\nRelease notes\n{notes}\n"));
    }
    text
}

/// An entry's requirements, as `beta>=1.0, gamma>2`.
fn requirements(entry: &Entry) -> String {
    let needs: Vec<String> = entry
        .requires
        .iter()
        .map(|r| match &r.min {
            Some(min) if r.min_exclusive => format!("{}>{min}", r.name),
            Some(min) => format!("{}>={min}", r.name),
            None => r.name.clone(),
        })
        .collect();
    needs.join(", ")
}

/// LabVIEW's year from its internal version, as core reads it (25.3 is 2025).
pub fn labview_year(internal: f64) -> u32 {
    2000 + internal.trunc() as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browse::Status;

    fn entry(version: &str) -> Entry {
        Entry {
            name: "alpha".into(),
            version: Version::parse(version),
            url: String::new(),
            md5: None,
            display_name: None,
            requires: Vec::new(),
            lv_min: Some(20.0),
            source: "https://feed.example".into(),
        }
    }

    fn detail() -> Detail {
        Detail::new(&Package {
            name: "alpha".into(),
            display_name: Some("Alpha Tools".into()),
            versions: vec![entry("2.0"), entry("1.0")],
            installed: None,
            available: None,
            status: Status::NotInstalled,
        })
    }

    #[test]
    fn stepping_stops_at_either_end_and_shows_the_fields_from_the_top() {
        let mut detail = detail();
        detail.scroll_by(5);
        detail.step(1);
        assert_eq!(detail.key(), ("alpha".to_string(), "1.0".to_string()));
        assert_eq!(detail.scroll, 0);
        detail.step(isize::MAX);
        assert_eq!(detail.table.selected(), Some(1));
        detail.step(isize::MIN);
        assert_eq!(detail.table.selected(), Some(0));
        detail.scroll_by(-3);
        assert_eq!(detail.scroll, 0);
    }

    #[test]
    fn the_fields_say_what_is_loading_or_what_failed() {
        let detail = detail();
        let loading = fields(&detail, None);
        assert!(loading.starts_with("Alpha Tools\n"), "{loading}");
        assert!(loading.contains("LabVIEW       2020 or newer"), "{loading}");
        assert!(
            loading.ends_with("Reading the details of alpha 2.0…"),
            "{loading}"
        );
        let failed = fields(&detail, Some(&Err("no route".into())));
        assert!(failed.ends_with("could not be read:\nno route"), "{failed}");
    }
}
