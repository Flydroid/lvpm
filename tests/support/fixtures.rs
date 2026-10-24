//! Fixture packages: `.vip` archives generated from a declarative table.
//!
//! Every zip entry carries the same fixed timestamp and entries are written in
//! a fixed order, with no compression, so an archive's bytes (and SHA-256) are
//! the same on every run and every machine.

use std::io::{Cursor, Write};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipWriter};

/// Where a fixture's payload file is installed (`<application>` is a scratch
/// prefix under `--prefix`).
const PAYLOAD_DIR: &str = "vi.lib/lvpm_fixture";
/// The archive member lvpm extracts for a declared PostInstall hook.
const HOOK_MEMBER: &str = "PostInstall.vi";
/// Stands in for a real script VI; lvpm never opens it under `--prefix`.
const HOOK_PLACEHOLDER: &[u8] = b"placeholder for a PostInstall script VI\n";
const ZIP_ENTRY_MODE: u32 = 0o644;

pub struct FixturePackage {
    pub name: &'static str,
    pub version: &'static str,
    pub display_name: &'static str,
    /// `(package, minimum version)` pairs, written as `package>=minimum`.
    pub requires: &'static [(&'static str, &'static str)],
    /// Minimum LabVIEW, e.g. `20.0`.
    pub lv_min: &'static str,
    /// The PostInstall script VI the spec declares, if any.
    pub post_install_hook: Option<&'static str>,
}

/// alpha 1.0 and 2.0, beta 1.0 requiring alpha >= 1.0, and hooked 1.0
/// declaring a PostInstall hook.
pub const PACKAGES: &[FixturePackage] = &[
    FixturePackage {
        name: "alpha",
        version: "1.0",
        display_name: "Alpha",
        requires: &[],
        lv_min: "20.0",
        post_install_hook: None,
    },
    FixturePackage {
        name: "alpha",
        version: "2.0",
        display_name: "Alpha",
        requires: &[],
        lv_min: "20.0",
        post_install_hook: None,
    },
    FixturePackage {
        name: "beta",
        version: "1.0",
        display_name: "Beta",
        requires: &[("alpha", "1.0")],
        lv_min: "20.0",
        post_install_hook: None,
    },
    FixturePackage {
        name: "hooked",
        version: "1.0",
        display_name: "Hooked",
        requires: &[],
        lv_min: "20.0",
        post_install_hook: Some(HOOK_MEMBER),
    },
];

impl FixturePackage {
    /// The id a package directory lists it under, e.g. `alpha-1.0`.
    pub fn id(&self) -> String {
        format!("{}-{}", self.name, self.version)
    }

    pub fn file_name(&self) -> String {
        format!("{}.vip", self.id())
    }

    /// `alpha>=1.0,gamma>=2.0`; empty when the package requires nothing.
    pub fn requires_clause(&self) -> String {
        let parts: Vec<String> = self
            .requires
            .iter()
            .map(|(name, min)| format!("{name}>={min}"))
            .collect();
        parts.join(",")
    }

    fn payload_path(&self) -> String {
        format!("{PAYLOAD_DIR}/{}.txt", self.name)
    }

    fn spec_text(&self) -> String {
        let mut spec = format!(
            "[Package]\nName=\"{}\"\nVersion=\"{}\"\nDisplay Name=\"{}\"\n\n",
            self.name, self.version, self.display_name
        );
        if !self.requires.is_empty() {
            spec.push_str(&format!(
                "[Dependencies]\nRequires=\"{}\"\n\n",
                self.requires_clause()
            ));
        }
        spec.push_str(&format!(
            "[Platform]\nExclusive_LabVIEW_Version=\"LabVIEW>={}\"\n\n",
            self.lv_min
        ));
        if let Some(hook) = self.post_install_hook {
            spec.push_str(&format!("[Script VIs]\nPostInstall=\"{hook}\"\n\n"));
        }
        spec.push_str(&format!(
            "[Files]\nNum File Groups=\"1\"\n\n[File Group 0]\nTarget Dir=\"<application>\"\n\
             Replace Mode=\"Always\"\nNum Files=1\nFile 0=\"{}\"\n",
            self.payload_path()
        ));
        spec
    }

    /// The `.vip` archive: `spec`, then the payload, then the hook VI.
    pub fn build_vip(&self) -> Vec<u8> {
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .last_modified_time(DateTime::default())
            .unix_permissions(ZIP_ENTRY_MODE);
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let mut add = |member: &str, bytes: &[u8]| {
            writer
                .start_file(member, options)
                .expect("start zip entry in memory");
            writer.write_all(bytes).expect("write zip entry in memory");
        };
        add("spec", self.spec_text().as_bytes());
        add(
            &format!("File Group 0/{}", self.payload_path()),
            format!("{} {}\n", self.name, self.version).as_bytes(),
        );
        if self.post_install_hook.is_some() {
            add(HOOK_MEMBER, HOOK_PLACEHOLDER);
        }
        writer.finish().expect("finish zip in memory").into_inner()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use zip::ZipArchive;

    fn package(name: &str, version: &str) -> &'static FixturePackage {
        PACKAGES
            .iter()
            .find(|p| p.name == name && p.version == version)
            .expect("in the table")
    }

    fn members(vip: &[u8]) -> Vec<(String, Option<DateTime>)> {
        let mut archive = ZipArchive::new(Cursor::new(vip)).expect("a valid zip");
        (0..archive.len())
            .map(|i| {
                let entry = archive.by_index(i).expect("entry");
                (entry.name().to_string(), entry.last_modified())
            })
            .collect()
    }

    #[test]
    fn building_the_same_table_twice_gives_byte_identical_archives() {
        for pkg in PACKAGES {
            assert_eq!(
                pkg.build_vip(),
                pkg.build_vip(),
                "{} differs between builds",
                pkg.id()
            );
        }
    }

    #[test]
    fn every_entry_has_the_1980_timestamp_and_entries_are_in_fixed_order() {
        let epoch = DateTime::from_date_and_time(1980, 1, 1, 0, 0, 0).unwrap();
        let hooked = members(&package("hooked", "1.0").build_vip());
        let names: Vec<&str> = hooked.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            [
                "spec",
                "File Group 0/vi.lib/lvpm_fixture/hooked.txt",
                "PostInstall.vi"
            ]
        );
        assert!(
            hooked.iter().all(|(_, time)| *time == Some(epoch)),
            "{hooked:?}"
        );
    }

    #[test]
    fn distinct_versions_of_a_package_are_distinct_archives() {
        assert_ne!(
            package("alpha", "1.0").build_vip(),
            package("alpha", "2.0").build_vip()
        );
    }

    #[test]
    fn the_spec_declares_requirements_and_hooks_only_where_the_table_does() {
        let spec_of = |name: &str, version: &str| {
            let mut archive =
                ZipArchive::new(Cursor::new(package(name, version).build_vip())).unwrap();
            let mut text = String::new();
            archive
                .by_name("spec")
                .unwrap()
                .read_to_string(&mut text)
                .unwrap();
            text
        };
        assert!(spec_of("beta", "1.0").contains("Requires=\"alpha>=1.0\""));
        assert!(spec_of("hooked", "1.0").contains("PostInstall=\"PostInstall.vi\""));
        let alpha = spec_of("alpha", "1.0");
        assert!(
            !alpha.contains("Requires") && !alpha.contains("PostInstall"),
            "{alpha}"
        );
    }
}
