//! The `lvpm.toml` a scenario's project has.

/// The manifest's file name, which is also what marks a directory as a project.
pub const MANIFEST_FILE: &str = "lvpm.toml";
const PROJECT_NAME: &str = "fixture";
const PROJECT_VERSION: &str = "0.1.0";

/// A project that reads only from the sources listed here, never the public
/// feeds.
#[derive(Debug, Default)]
pub struct ProjectManifest {
    labview_year: Option<String>,
    sources: Vec<(String, String)>,
}

impl ProjectManifest {
    pub fn new() -> ProjectManifest {
        ProjectManifest::default()
    }

    /// The oldest LabVIEW the project is meant for.
    pub fn labview(mut self, year: &str) -> ProjectManifest {
        self.labview_year = Some(year.to_string());
        self
    }

    pub fn source(mut self, name: &str, url: &str) -> ProjectManifest {
        self.sources.push((name.to_string(), url.to_string()));
        self
    }

    pub fn render(&self) -> String {
        let mut text =
            format!("[project]\nname = \"{PROJECT_NAME}\"\nversion = \"{PROJECT_VERSION}\"\n");
        if let Some(year) = &self.labview_year {
            text.push_str(&format!("labview = \"{year}\"\n"));
        }
        text.push_str("\n[sources]\n");
        for (name, url) in &self.sources {
            text.push_str(&format!("{name} = \"{url}\"\n"));
        }
        text.push_str("defaults = false\n");
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_manifest_lists_its_sources_and_turns_the_public_feeds_off() {
        let text = ProjectManifest::new()
            .source("loopback", "http://127.0.0.1:1/feed")
            .render();
        assert_eq!(
            text,
            "[project]\nname = \"fixture\"\nversion = \"0.1.0\"\n\n[sources]\nloopback = \"http://127.0.0.1:1/feed\"\ndefaults = false\n"
        );
    }

    #[test]
    fn the_labview_year_goes_in_the_project_table_before_the_sources() {
        let text = ProjectManifest::new()
            .labview("2026")
            .source("a", "http://h/a")
            .render();
        let project = text.find("labview = \"2026\"").unwrap();
        assert!(project < text.find("[sources]").unwrap(), "{text}");
    }

    #[test]
    fn every_line_starts_in_the_first_column() {
        let text = ProjectManifest::new()
            .labview("2026")
            .source("a", "http://h/a")
            .render();
        assert!(text.lines().all(|line| !line.starts_with(' ')), "{text:?}");
    }

    #[test]
    fn a_rendered_manifest_is_valid_toml_with_the_declared_tables() {
        let text = ProjectManifest::new()
            .labview("2026")
            .source("a", "http://h/a")
            .render();
        let parsed: toml::Table = text.parse().unwrap();
        assert_eq!(parsed["project"]["labview"].as_str(), Some("2026"));
        assert_eq!(parsed["sources"]["a"].as_str(), Some("http://h/a"));
        assert_eq!(parsed["sources"]["defaults"].as_bool(), Some(false));
    }
}
