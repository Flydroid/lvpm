//! Fixture packages: `.vip` archives generated from a declarative table.
//!
//! Every zip entry carries the same fixed timestamp and entries are written in
//! a fixed order, with no compression, so an archive's bytes (and SHA-256) are
//! the same on every run and every machine.

use std::io::{Cursor, Write};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, System, ZipWriter};

/// Where a text fixture's payload file is installed (`<application>` is a
/// scratch prefix under `--prefix`).
const TEXT_PAYLOAD_DIR: &str = "vi.lib/lvpm_fixture";
/// The file name of a real VI payload inside its directory.
const VI_PAYLOAD_FILE: &str = "payload.vi";
/// Where the venv fixture's VI is installed.
const VENV_VI_PAYLOAD_DIR: &str = "vi.lib/lvpm_vi_fixture";
/// The only folder the LabVIEW-installation fixture may write to, below the
/// installation's directory.
pub const GLOBAL_PAYLOAD_DIR: &str = "vi.lib/_lvpm_test_fixture";
/// A real VI to relink: the relink VI itself, saved under another name, so it
/// never collides with the copy lvpm runs.
const REAL_VI: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/crates/lvpm-core/src/lv-src/relink-package.vi"
));
/// A real PostInstall hook: reads the `Package Name` attribute of its `Variant`
/// control and writes it to `lvpm-test-hook-marker.txt` in LabVIEW's temp dir.
const REAL_HOOK: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/hooks/PostInstall.vi"
));
/// The archive member lvpm extracts for a declared PostInstall hook.
const HOOK_MEMBER: &str = "PostInstall.vi";
/// Stands in for a real script VI; lvpm never opens it under `--prefix`.
const HOOK_PLACEHOLDER: &[u8] = b"placeholder for a PostInstall script VI\n";
const ZIP_ENTRY_MODE: u32 = 0o644;

/// What a package installs.
#[derive(Debug)]
pub enum Payload {
    /// One text file, `vi.lib/lvpm_fixture/<name>.txt`.
    Text,
    /// One real VI, `<dir>/payload.vi`.
    Vi {
        dir: &'static str,
        bytes: &'static [u8],
    },
}

/// A PostInstall script VI: the path the spec declares, and the file shipped.
#[derive(Debug)]
pub struct HookVi {
    pub declared: &'static str,
    pub bytes: &'static [u8],
}

#[derive(Debug)]
pub struct FixturePackage {
    pub name: &'static str,
    pub version: &'static str,
    pub display_name: &'static str,
    /// `(package, minimum version)` pairs, written as `package>=minimum`.
    pub requires: &'static [(&'static str, &'static str)],
    /// Minimum LabVIEW, e.g. `20.0`.
    pub lv_min: &'static str,
    pub payload: Payload,
    /// The PostInstall script VI the spec declares, if any.
    pub post_install_hook: Option<HookVi>,
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
        payload: Payload::Text,
        post_install_hook: None,
    },
    FixturePackage {
        name: "alpha",
        version: "2.0",
        display_name: "Alpha",
        requires: &[],
        lv_min: "20.0",
        payload: Payload::Text,
        post_install_hook: None,
    },
    FixturePackage {
        name: "beta",
        version: "1.0",
        display_name: "Beta",
        requires: &[("alpha", "1.0")],
        lv_min: "20.0",
        payload: Payload::Text,
        post_install_hook: None,
    },
    FixturePackage {
        name: "hooked",
        version: "1.0",
        display_name: "Hooked",
        requires: &[],
        lv_min: "20.0",
        payload: Payload::Text,
        post_install_hook: Some(HookVi {
            declared: HOOK_MEMBER,
            bytes: HOOK_PLACEHOLDER,
        }),
    },
];

/// What a plain text fixture needs that most do not state: a package with this
/// name and version, requiring `requires`, for LabVIEW `lv_min` and newer.
const fn text_package(
    name: &'static str,
    version: &'static str,
    requires: &'static [(&'static str, &'static str)],
    lv_min: &'static str,
) -> FixturePackage {
    FixturePackage {
        name,
        version,
        display_name: name,
        requires,
        lv_min,
        payload: Payload::Text,
        post_install_hook: None,
    }
}

/// What the `lvpm lock` scenarios publish: alpha 1.0 and 2.0, beta 1.0
/// requiring alpha >= 1.0, gamma 1.0 and 1.5 (which needs LabVIEW 2024), and
/// caraya 1.0.
pub const LOCK_PACKAGES: &[FixturePackage] = &[
    text_package("alpha", "1.0", &[], "20.0"),
    text_package("alpha", "2.0", &[], "20.0"),
    text_package("beta", "1.0", &[("alpha", "1.0")], "20.0"),
    text_package("gamma", "1.0", &[], "20.0"),
    text_package("gamma", "1.5", &[], "24.0"),
    text_package("caraya", "1.0", &[], "20.0"),
];

/// What the exact-sync scenarios publish: alpha 1.0 and 2.0, beta 1.0
/// requiring alpha >= 1.0, gamma 1.0, caraya 1.0 requiring delta >= 1.0, and
/// delta 1.0.
pub const SYNC_PACKAGES: &[FixturePackage] = &[
    text_package("alpha", "1.0", &[], "20.0"),
    text_package("alpha", "2.0", &[], "20.0"),
    text_package("beta", "1.0", &[("alpha", "1.0")], "20.0"),
    text_package("gamma", "1.0", &[], "20.0"),
    text_package("caraya", "1.0", &[("delta", "1.0")], "20.0"),
    text_package("delta", "1.0", &[], "20.0"),
];

/// The same caraya 1.0, also requiring alpha >= 1.0.
pub const CARAYA_NEEDING_ALPHA: FixturePackage = text_package(
    "caraya",
    "1.0",
    &[("delta", "1.0"), ("alpha", "1.0")],
    "20.0",
);

/// The same beta 1.0, now requiring alpha >= 2.0.
pub const BETA_NEEDING_ALPHA_2: FixturePackage =
    text_package("beta", "1.0", &[("alpha", "2.0")], "20.0");

/// The same alpha 2.0 as the table's, built with another display name: it has
/// other bytes, and so another SHA-256.
pub const ALTERED_ALPHA_2: FixturePackage = FixturePackage {
    name: "alpha",
    version: "2.0",
    display_name: "Alpha, altered",
    requires: &[],
    lv_min: "20.0",
    payload: Payload::Text,
    post_install_hook: None,
};

/// Releases a scenario publishes after a project was locked.
pub const LATER_RELEASES: &[FixturePackage] =
    &[text_package("beta", "3.0", &[("alpha", "1.0")], "20.0")];

/// What the upgrade scenarios publish: alpha 1.0 and 2.0, beta 0.9, 1.0, 1.5
/// and 2.0 each requiring alpha >= 1.0, and gamma 1.0 and 2.0.
pub const UPGRADE_PACKAGES: &[FixturePackage] = &[
    text_package("alpha", "1.0", &[], "20.0"),
    text_package("alpha", "2.0", &[], "20.0"),
    text_package("beta", "0.9", &[("alpha", "1.0")], "20.0"),
    text_package("beta", "1.0", &[("alpha", "1.0")], "20.0"),
    text_package("beta", "1.5", &[("alpha", "1.0")], "20.0"),
    text_package("beta", "2.0", &[("alpha", "1.0")], "20.0"),
    text_package("gamma", "1.0", &[], "20.0"),
    text_package("gamma", "2.0", &[], "20.0"),
];

/// What the add and remove scenarios publish: alpha 1.0 and 2.0, beta 1.0
/// requiring alpha >= 1.0, gamma 0.5 and 1.0, and caraya 1.0.
pub const ADD_PACKAGES: &[FixturePackage] = &[
    text_package("alpha", "1.0", &[], "20.0"),
    text_package("alpha", "2.0", &[], "20.0"),
    text_package("beta", "1.0", &[("alpha", "1.0")], "20.0"),
    text_package("gamma", "0.5", &[], "20.0"),
    text_package("gamma", "1.0", &[], "20.0"),
    text_package("caraya", "1.0", &[], "20.0"),
];

/// A delta 1.0, which the add scenarios install by hand, outside the lock.
pub const DELTA_1: FixturePackage = text_package("delta", "1.0", &[], "20.0");

/// An omega 1.0, the only omega, which needs LabVIEW 2026.
pub const OMEGA_NEEDING_LABVIEW_2026: FixturePackage = text_package("omega", "1.0", &[], "26.0");

/// An omega 1.0, the only omega, which requires alpha >= 2.0.
pub const OMEGA_NEEDING_ALPHA_2: FixturePackage =
    text_package("omega", "1.0", &[("alpha", "2.0")], "20.0");

/// An alpha 3.0 that needs LabVIEW 2026.
pub const ALPHA_3_NEEDING_LABVIEW_2026: FixturePackage = text_package("alpha", "3.0", &[], "26.0");

/// The same beta 2.0, now requiring alpha >= 2.0.
pub const BETA_2_NEEDING_ALPHA_2: FixturePackage =
    text_package("beta", "2.0", &[("alpha", "2.0")], "20.0");

/// The same beta 2.0, now needing LabVIEW 2026.
pub const BETA_2_NEEDING_LABVIEW_2026: FixturePackage =
    text_package("beta", "2.0", &[("alpha", "1.0")], "26.0");

/// The same beta 1.0, also requiring a package that no source lists.
pub const BETA_NEEDING_A_MISSING_PACKAGE: FixturePackage = text_package(
    "beta",
    "1.0",
    &[("alpha", "1.0"), ("ni_lib_missing", "1.0")],
    "20.0",
);

/// A catalog greedy picking fails on: A 2.0 wants C >= 2.0, and only C 1.0
/// exists.
pub const CONFLICTING_PACKAGES: &[FixturePackage] = &[
    text_package("A", "2.0", &[("C", "2.0")], "20.0"),
    text_package("C", "1.0", &[], "20.0"),
];

/// Packages with real VIs. They are not in any feed: a scenario puts one in
/// the sandbox's local package folder, so no recording of a search sees them.
pub const VI_PACKAGES: &[FixturePackage] = &[
    // Installed into a venv, where `relink` really relinks its VI.
    FixturePackage {
        name: "lvpm_vi_fixture",
        version: "1.0",
        display_name: "LVPM VI Fixture",
        requires: &[],
        lv_min: "20.0",
        payload: Payload::Vi {
            dir: VENV_VI_PAYLOAD_DIR,
            bytes: REAL_VI,
        },
        post_install_hook: None,
    },
    // Installed into the LabVIEW installation: a VI to relink under
    // `vi.lib/_lvpm_test_fixture`, and a hook that leaves a marker.
    FixturePackage {
        name: "lvpm_test_fixture",
        version: "1.0",
        display_name: "LVPM Test Fixture",
        requires: &[],
        lv_min: "20.0",
        payload: Payload::Vi {
            dir: GLOBAL_PAYLOAD_DIR,
            bytes: REAL_VI,
        },
        post_install_hook: Some(HookVi {
            declared: HOOK_MEMBER,
            bytes: REAL_HOOK,
        }),
    },
];

impl FixturePackage {
    /// The table's package with this name and version.
    pub fn find(name: &str, version: &str) -> &'static FixturePackage {
        PACKAGES
            .iter()
            .chain(VI_PACKAGES)
            .find(|package| package.name == name && package.version == version)
            .unwrap_or_else(|| panic!("the fixture tables have no {name} {version}"))
    }

    /// The package with real VIs of this name, if there is one.
    pub fn find_vi_package(name: &str) -> Option<&'static FixturePackage> {
        VI_PACKAGES.iter().find(|package| package.name == name)
    }

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

    /// The payload's path below `<application>`, as the spec lists it.
    pub fn payload_path(&self) -> String {
        match &self.payload {
            Payload::Text => format!("{TEXT_PAYLOAD_DIR}/{}.txt", self.name),
            Payload::Vi { dir, .. } => format!("{dir}/{VI_PAYLOAD_FILE}"),
        }
    }

    fn payload_bytes(&self) -> Vec<u8> {
        match &self.payload {
            Payload::Text => format!("{} {}\n", self.name, self.version).into_bytes(),
            Payload::Vi { bytes, .. } => bytes.to_vec(),
        }
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
        if let Some(hook) = &self.post_install_hook {
            spec.push_str(&format!(
                "[Script VIs]\nPostInstall=\"{}\"\n\n",
                hook.declared
            ));
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
        self.build_vip_with("")
    }

    /// [`FixturePackage::build_vip`] with `extra_spec` appended to the spec.
    pub fn build_vip_with(&self, extra_spec: &str) -> Vec<u8> {
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .last_modified_time(DateTime::default())
            // The "made by" byte defaults to the host OS; pin it so the digest is the same everywhere.
            .system(System::Unix)
            .unix_permissions(ZIP_ENTRY_MODE);
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let mut add = |member: &str, bytes: &[u8]| {
            writer
                .start_file(member, options)
                .expect("start zip entry in memory");
            writer.write_all(bytes).expect("write zip entry in memory");
        };
        add(
            "spec",
            format!("{}{extra_spec}", self.spec_text()).as_bytes(),
        );
        add(
            &format!("File Group 0/{}", self.payload_path()),
            &self.payload_bytes(),
        );
        if let Some(hook) = &self.post_install_hook {
            add(HOOK_MEMBER, hook.bytes);
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
        FixturePackage::find(name, version)
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

    fn member_bytes(vip: &[u8], name: &str) -> Vec<u8> {
        let mut archive = ZipArchive::new(Cursor::new(vip)).expect("a valid zip");
        let mut bytes = Vec::new();
        archive
            .by_name(name)
            .unwrap_or_else(|_| panic!("no member {name}"))
            .read_to_end(&mut bytes)
            .unwrap();
        bytes
    }

    #[test]
    fn the_real_vis_are_labview_files() {
        for vi in [REAL_VI, REAL_HOOK] {
            assert_eq!(&vi[..4], b"RSRC", "not a LabVIEW resource file");
        }
    }

    #[test]
    fn the_vi_packages_are_not_in_the_feed_table_and_build_the_same_every_time() {
        for pkg in VI_PACKAGES {
            assert!(PACKAGES.iter().all(|feed| feed.name != pkg.name));
            assert_eq!(pkg.build_vip(), pkg.build_vip(), "{} differs", pkg.id());
        }
    }

    #[test]
    fn the_venv_fixture_ships_the_real_vi_and_no_hook() {
        let vip = package("lvpm_vi_fixture", "1.0").build_vip();
        let payload = member_bytes(&vip, "File Group 0/vi.lib/lvpm_vi_fixture/payload.vi");
        assert!(payload == REAL_VI, "the payload is not the real VI");
        let names: Vec<String> = members(&vip).into_iter().map(|(name, _)| name).collect();
        assert!(!names.contains(&"PostInstall.vi".to_string()), "{names:?}");
    }

    #[test]
    fn the_installation_fixture_writes_only_below_its_own_folder_and_ships_the_real_hook() {
        let fixture = package("lvpm_test_fixture", "1.0");
        assert!(
            fixture
                .payload_path()
                .starts_with("vi.lib/_lvpm_test_fixture/"),
            "{}",
            fixture.payload_path()
        );
        let vip = fixture.build_vip();
        assert!(
            member_bytes(&vip, "PostInstall.vi") == REAL_HOOK,
            "the hook is not the real hook VI"
        );
        let spec = String::from_utf8(member_bytes(&vip, "spec")).unwrap();
        assert!(spec.contains("Num Files=1"), "{spec}");
        assert!(spec.contains("PostInstall=\"PostInstall.vi\""), "{spec}");
        assert_eq!(
            spec.matches("File ").count(),
            spec.matches("File Group").count() + spec.matches("File 0=").count(),
            "only one payload file is declared: {spec}"
        );
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
    #[should_panic(expected = "no alpha 9.9")]
    fn finding_a_package_that_is_not_in_the_table_is_an_error() {
        FixturePackage::find("alpha", "9.9");
    }

    /// The archive is the same on every machine. If the zip crate or the
    /// builder changes its bytes, every recording that shows a hash changes
    /// too (`cache-add`), so update both deliberately.
    #[test]
    fn alpha_1_0_has_a_pinned_sha256() {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(package("alpha", "1.0").build_vip());
        let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(
            hex,
            "70aa2ae4a939974873000c5bfdac3ce49484d2f251ddeafd9583c3529349ce22"
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
    fn the_lock_fixtures_are_built_the_same_every_time_and_no_two_are_alike() {
        let mut archives = std::collections::HashSet::new();
        for pkg in LOCK_PACKAGES
            .iter()
            .chain(CONFLICTING_PACKAGES)
            .chain([&BETA_NEEDING_A_MISSING_PACKAGE])
        {
            assert_eq!(pkg.build_vip(), pkg.build_vip(), "{} differs", pkg.id());
            assert!(archives.insert(pkg.build_vip()), "{} repeats", pkg.id());
        }
    }

    #[test]
    fn the_upgrade_fixtures_are_built_the_same_every_time_and_no_two_are_alike() {
        let mut archives = std::collections::HashSet::new();
        for pkg in UPGRADE_PACKAGES.iter().chain([
            &ALPHA_3_NEEDING_LABVIEW_2026,
            &BETA_2_NEEDING_ALPHA_2,
            &BETA_2_NEEDING_LABVIEW_2026,
        ]) {
            assert_eq!(pkg.build_vip(), pkg.build_vip(), "{} differs", pkg.id());
            assert!(archives.insert(pkg.build_vip()), "{} repeats", pkg.id());
        }
    }

    #[test]
    fn gamma_1_5_needs_labview_2024_and_the_variant_beta_wants_a_missing_package() {
        let spec_of = |pkg: &FixturePackage| {
            let mut archive = ZipArchive::new(Cursor::new(pkg.build_vip())).unwrap();
            let mut text = String::new();
            archive
                .by_name("spec")
                .unwrap()
                .read_to_string(&mut text)
                .unwrap();
            text
        };
        let gamma = LOCK_PACKAGES
            .iter()
            .find(|pkg| pkg.name == "gamma" && pkg.version == "1.5")
            .unwrap();
        assert!(spec_of(gamma).contains("LabVIEW>=24.0"));
        assert!(
            spec_of(&BETA_NEEDING_A_MISSING_PACKAGE)
                .contains("Requires=\"alpha>=1.0,ni_lib_missing>=1.0\"")
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
