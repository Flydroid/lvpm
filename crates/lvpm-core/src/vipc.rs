//! Reading a VIPM `.vipc`: a zip whose `config.xml` lists the packages of one
//! LabVIEW target. The file is untrusted input, so the reader takes only what
//! it understands from the one `<Target>` and refuses everything else, with an
//! error that names the file. No XML dependency: the file is small and flat.

use crate::lvpkg::check_name;
use crate::ops::init::Seed;
use crate::project::same_name;
use crate::version::is_version;
use anyhow::{Result, anyhow};
use std::fs::File;
use std::io::Read;
use std::path::Path;
use zip::ZipArchive;
use zip::result::ZipError;

/// The most of `config.xml` that is read; a real one is a few KiB.
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

/// One `<Package>` of the target.
#[derive(Debug, Clone, PartialEq)]
pub struct Package {
    pub name: String,
    pub version: String,
    /// The `.vip` is inside the `.vipc`.
    pub included: bool,
    /// VIPM only lists it because another package needs it.
    pub dependency: bool,
    /// The user fixed this version.
    pub pinned: bool,
}

/// What a `.vipc` says.
#[derive(Debug, Clone, PartialEq)]
pub struct Vipc {
    /// The year of the LabVIEW it targets: `13.0` is 2013.
    pub labview_year: u32,
    pub packages: Vec<Package>,
}

/// Read the `.vipc` at `path`. Every error starts with `label`, the name the
/// user knows the file by (`--from` as typed, which `path` may have been resolved from).
pub fn read(path: &Path, label: &str) -> Result<Vipc> {
    let config = read_config(path).map_err(|what| anyhow!("{label}: {what}"))?;
    parse(&config).map_err(|what| anyhow!("{label}: config.xml: {what}"))
}

/// The text of the archive's `config.xml`; the error is what is wrong with the file.
fn read_config(path: &Path) -> Result<String, String> {
    let file = File::open(path).map_err(|e| format!("cannot be read: {e}"))?;
    let mut zip = ZipArchive::new(file).map_err(|e| match e {
        ZipError::Io(e) => format!("cannot be read: {e}"),
        _ => "not a zip file".to_string(),
    })?;
    let entry = zip.by_name("config.xml").map_err(|e| match e {
        ZipError::FileNotFound => "config.xml is missing".to_string(),
        e => format!("config.xml: {e}"),
    })?;
    let mut bytes = Vec::new();
    entry
        .take(MAX_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("config.xml: {e}"))?;
    if bytes.len() as u64 > MAX_CONFIG_BYTES {
        return Err("config.xml: larger than 1 MiB".to_string());
    }
    String::from_utf8(bytes).map_err(|_| "config.xml: not UTF-8 text".to_string())
}

/// The text of `config.xml` read as a `Vipc`; the error is what is wrong with it.
fn parse(xml: &str) -> Result<Vipc, String> {
    let target = target(xml)?;
    let (head, mut rest) = target.split_at(target.find("<Package>").unwrap_or(target.len()));
    let labview_year = year(field(head, "Version", "")?)?;
    let mut packages = Vec::new();
    while let Some((block, after)) = element(rest, "Package") {
        packages.push(package(block)?);
        rest = after;
    }
    if rest.contains("<Package>") {
        return Err("<Package> is not closed".to_string());
    }
    Ok(Vipc {
        labview_year,
        packages,
    })
}

/// The inside of the one `<Target>`, the region everything else is read from.
fn target(xml: &str) -> Result<&str, String> {
    match xml.matches("<Target>").count() {
        0 => return Err("no <Target>".to_string()),
        1 => {}
        _ => return Err("more than one <Target>".to_string()),
    }
    let (inside, _) = element(xml, "Target").ok_or("<Target> is not closed")?;
    for (marker, construct) in [("<!--", "comment"), ("<![CDATA[", "CDATA"), ("&", "entity")] {
        if inside.contains(marker) {
            return Err(format!("unsupported XML ({construct})"));
        }
    }
    Ok(inside)
}

fn package(block: &str) -> Result<Package, String> {
    let full = field(block, "Name", "package: ")?;
    let (name, version) = full
        .rsplit_once('-')
        .ok_or_else(|| format!("package {full}: no version (expected name-version)"))?;
    if !is_version(version) {
        return Err(format!("package {full}: {version} is not a version"));
    }
    check_name(name).map_err(|_| format!("package {full}: {name} is not a package name"))?;
    let who = format!("package {name}: ");
    Ok(Package {
        name: name.to_string(),
        version: version.to_string(),
        included: flag(block, "Included", &who)?,
        dependency: flag(block, "Dependency", &who)?,
        pinned: flag(block, "Pinned", &who)?,
    })
}

/// `13.0` is 2013: a two-digit major, a dot, one or more digits, nothing else.
fn year(value: &str) -> Result<u32, String> {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    match value.split_once('.') {
        Some((major, minor)) if major.len() == 2 && digits(major) && digits(minor) => major
            .parse::<u32>()
            .map(|major| 2000 + major)
            .map_err(|e| e.to_string()),
        _ => Err(format!("<Version> {value} is not N.N")),
    }
}

/// `<tag>` read as `true` or `false` in any case. `who` leads an error.
fn flag(block: &str, tag: &str, who: &str) -> Result<bool, String> {
    let value = field(block, tag, who)?;
    match value.to_ascii_lowercase().as_str() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(format!("{who}{tag} is not true or false")),
    }
}

/// The text of the first `<tag>` in `block`. `who` leads the error of a missing one.
fn field<'a>(block: &'a str, tag: &str, who: &str) -> Result<&'a str, String> {
    element(block, tag)
        .map(|(inside, _)| inside)
        .ok_or_else(|| format!("{who}{tag} is missing"))
}

/// The first `<tag>...</tag>` in `s`: what is inside, and what follows it.
fn element<'a>(s: &'a str, tag: &str) -> Option<(&'a str, &'a str)> {
    let open = format!("<{tag}>");
    let from = s.find(&open)? + open.len();
    let len = s[from..].find(&format!("</{tag}>"))?;
    Some((&s[from..from + len], &s[from + len + tag.len() + 3..]))
}

impl Vipc {
    /// What init takes from it. A `Dependency` package is skipped and counted;
    /// a `Pinned` one gets its exact version and the rest a floor. A name listed
    /// twice keeps the last one's constraint and counts once.
    pub fn seed(&self) -> Seed {
        let mut seed = Seed {
            source_labview: Some(self.labview_year),
            ..Seed::default()
        };
        for package in &self.packages {
            if package.dependency {
                seed.skipped += 1;
                continue;
            }
            let constraint = match package.pinned {
                true => package.version.clone(),
                false => format!(">={}", package.version),
            };
            match seed
                .deps
                .iter_mut()
                .find(|(name, _)| same_name(name, &package.name))
            {
                Some(dep) => dep.1 = constraint,
                None => seed.deps.push((package.name.clone(), constraint)),
            }
        }
        seed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    /// A temp folder, removed on drop.
    struct Dir(std::path::PathBuf);

    impl Dir {
        fn new(tag: &str) -> Dir {
            let dir =
                std::env::temp_dir().join(format!("lvpm-vipc-test-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Dir(dir)
        }

        /// `p.vipc` in the folder, a zip holding `entries`.
        fn vipc(&self, entries: &[(&str, &[u8])]) -> std::path::PathBuf {
            let path = self.0.join("p.vipc");
            let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
            for (name, bytes) in entries {
                zip.start_file(*name, SimpleFileOptions::default()).unwrap();
                zip.write_all(bytes).unwrap();
            }
            zip.finish().unwrap();
            path
        }

        fn config(&self, xml: &str) -> std::path::PathBuf {
            self.vipc(&[("config.xml", xml.as_bytes())])
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn package_xml(name: &str, included: &str, dependency: &str, pinned: &str) -> String {
        format!(
            "    <Package>\r\n      <Name>{name}</Name>\r\n      <Included>{included}</Included>\r\n      \
             <Dependency>{dependency}</Dependency>\r\n      <Pinned>{pinned}</Pinned>\r\n    </Package>\r\n"
        )
    }

    fn config_xml(version: &str, packages: &str) -> String {
        format!(
            "<VI_Package_Configuration File_Type=\"xml\" Version=\"0.6\">\r\n  <Target>\r\n    \
             <Name>LabVIEW</Name>\r\n    <Version>{version}</Version>\r\n{packages}  </Target>\r\n\
             </VI_Package_Configuration>"
        )
    }

    /// The one package `demo_lib-1.5.0.15`, as chosen and not pinned.
    fn demo(packages: &str) -> String {
        config_xml("13.0", packages)
    }

    fn chosen(name: &str) -> String {
        package_xml(name, "false", "false", "false")
    }

    /// What reading `xml` says: the `Vipc`, or the whole error text.
    fn read_xml(tag: &str, xml: &str) -> Result<Vipc, String> {
        let dir = Dir::new(tag);
        let path = dir.config(xml);
        read(&path, &path.display().to_string()).map_err(|e| e.to_string())
    }

    #[test]
    fn the_serializer_sample_parses() {
        let xml = config_xml(
            "13.0",
            &(package_xml(
                "labview_open_source_lib_data_manipulation-1.5.0.15",
                "false",
                "true",
                "false",
            ) + &package_xml(
                "labview_open_source_project_lib_labview_open_source_palette-1.0.2.7",
                "false",
                "false",
                "false",
            )),
        );
        let vipc = read_xml("sample", &xml).unwrap();
        assert_eq!(vipc.labview_year, 2013);
        assert_eq!(
            vipc.packages,
            [
                Package {
                    name: "labview_open_source_lib_data_manipulation".into(),
                    version: "1.5.0.15".into(),
                    included: false,
                    dependency: true,
                    pinned: false,
                },
                Package {
                    name: "labview_open_source_project_lib_labview_open_source_palette".into(),
                    version: "1.0.2.7".into(),
                    included: false,
                    dependency: false,
                    pinned: false,
                },
            ]
        );
    }

    #[test]
    fn what_the_reader_accepts() {
        let one = |xml: String| read_xml("accepts", &xml).unwrap();
        let split = one(demo(&chosen("a_b-c_d-1.2.3.4")));
        assert_eq!(
            (
                split.packages[0].name.as_str(),
                split.packages[0].version.as_str()
            ),
            ("a_b-c_d", "1.2.3.4")
        );
        assert_eq!(one(config_xml("26.3", "")).labview_year, 2026);
        assert_eq!(one(config_xml("13.10", "")).labview_year, 2013);
        let any_case = one(demo(&package_xml(
            "demo_lib-1.0.0.1",
            "True",
            "FALSE",
            "tRuE",
        )));
        let p = &any_case.packages[0];
        assert_eq!((p.included, p.dependency, p.pinned), (true, false, true));
        assert!(one(demo("")).packages.is_empty());
    }

    #[test]
    fn what_the_reader_refuses_names_the_vipc_and_the_reason() {
        let dir = Dir::new("refuses");
        let path = dir.0.join("p.vipc");
        let no_pinned = "    <Package>\r\n      <Name>demo_lib-1.0.0.1</Name>\r\n      <Included>false</Included>\r\n      \
                         <Dependency>false</Dependency>\r\n    </Package>\r\n";
        let cases: Vec<(String, String)> = vec![
            ("<Root/>".into(), "no <Target>".into()),
            (format!("{0}{0}", demo("")), "more than one <Target>".into()),
            (
                "<Target><Version>13.0</Version>".into(),
                "<Target> is not closed".into(),
            ),
            (
                demo(&chosen("foo")),
                "package foo: no version (expected name-version)".into(),
            ),
            (
                demo(&chosen("foo-bar")),
                "package foo-bar: bar is not a version".into(),
            ),
            (
                demo(&chosen("-1.0.0.1")),
                "package -1.0.0.1:  is not a package name".into(),
            ),
            (
                demo(&chosen("a b-1.0.0.1")),
                "package a b-1.0.0.1: a b is not a package name".into(),
            ),
            (
                demo(&chosen("..-1.0.0.1")),
                "package ..-1.0.0.1: .. is not a package name".into(),
            ),
            (
                demo(no_pinned),
                "package demo_lib: Pinned is missing".into(),
            ),
            (
                demo(&package_xml("demo_lib-1.0.0.1", "yes", "false", "false")),
                "package demo_lib: Included is not true or false".into(),
            ),
            (
                demo(&package_xml("demo_lib-1.0.0.1", "false", " true", "false")),
                "package demo_lib: Dependency is not true or false".into(),
            ),
            (
                demo("    <Package>\r\n    </Package>\r\n"),
                "package: Name is missing".into(),
            ),
            (demo("    <Package>\r\n"), "<Package> is not closed".into()),
            (
                config_xml("thirteen", ""),
                "<Version> thirteen is not N.N".into(),
            ),
            (
                config_xml("13.0.1", ""),
                "<Version> 13.0.1 is not N.N".into(),
            ),
            (config_xml("v13.0", ""), "<Version> v13.0 is not N.N".into()),
            (
                config_xml("2013.0", ""),
                "<Version> 2013.0 is not N.N".into(),
            ),
            (config_xml("13.", ""), "<Version> 13. is not N.N".into()),
            (config_xml("1.0", ""), "<Version> 1.0 is not N.N".into()),
            (config_xml("+3.0", ""), "<Version> +3.0 is not N.N".into()),
            ("<Target></Target>".into(), "Version is missing".into()),
            (
                demo("    <!-- hidden -->\r\n"),
                "unsupported XML (comment)".into(),
            ),
            (
                demo("    <![CDATA[x]]>\r\n"),
                "unsupported XML (CDATA)".into(),
            ),
            (
                demo(&chosen("a&amp;b-1.0.0.1")),
                "unsupported XML (entity)".into(),
            ),
        ];
        for (xml, what) in cases {
            let zip = dir.config(&xml);
            let err = read(&zip, &path.display().to_string())
                .unwrap_err()
                .to_string();
            assert_eq!(
                err,
                format!("{}: config.xml: {what}", path.display()),
                "for {xml}"
            );
        }
    }

    #[test]
    fn text_outside_the_target_is_never_looked_at() {
        let xml = demo(&chosen("demo_lib-1.0.0.1")).replace(
            "<VI_Package_Configuration",
            "<!-- a & b <![CDATA[ x ]]> --><VI_Package_Configuration",
        );
        assert_eq!(read_xml("outside", &xml).unwrap().packages.len(), 1);
    }

    #[test]
    fn config_xml_is_read_up_to_1_mib_and_no_further() {
        let padded = |bytes: usize| {
            let xml = demo(&chosen("demo_lib-1.0.0.1"));
            let pad = bytes - xml.len() - "<!---->".len();
            xml.replace("<Target>", &format!("<!--{}--><Target>", " ".repeat(pad)))
        };
        let at_limit = padded(1024 * 1024);
        assert_eq!(at_limit.len(), 1024 * 1024);
        assert!(read_xml("at-limit", &at_limit).is_ok());
        let err = read_xml("over-limit", &padded(2 * 1024 * 1024)).unwrap_err();
        assert!(
            err.ends_with("p.vipc: config.xml: larger than 1 MiB"),
            "{err}"
        );
    }

    #[test]
    fn a_file_that_is_no_vipc_is_refused_naming_it() {
        let dir = Dir::new("not-vipc");
        let path = dir.0.join("p.vipc");
        let shown = path.display();

        let missing = read(&path, &shown.to_string()).unwrap_err().to_string();
        assert!(
            missing.starts_with(&format!("{shown}: cannot be read: ")),
            "{missing}"
        );

        std::fs::write(&path, "hello").unwrap();
        assert_eq!(
            read(&path, &shown.to_string()).unwrap_err().to_string(),
            format!("{shown}: not a zip file")
        );

        dir.vipc(&[("readme.txt", b"x")]);
        assert_eq!(
            read(&path, &shown.to_string()).unwrap_err().to_string(),
            format!("{shown}: config.xml is missing")
        );

        dir.vipc(&[("config.xml", &[0xff, 0xfe])]);
        assert_eq!(
            read(&path, &shown.to_string()).unwrap_err().to_string(),
            format!("{shown}: config.xml: not UTF-8 text")
        );
    }

    #[test]
    fn an_error_leads_with_the_label_given_not_with_the_path_read() {
        let dir = Dir::new("label");
        let path = dir.0.join("p.vipc");
        std::fs::write(&path, "hello").unwrap();

        let err = read(&path, "as/typed.vipc").unwrap_err().to_string();

        assert_eq!(err, "as/typed.vipc: not a zip file");
    }

    #[test]
    fn seed_applies_the_rules() {
        let packages = [
            package_xml("chosen_pinned-1.0.0.1", "false", "false", "true"),
            package_xml("chosen_floating-2.0.0.2", "false", "false", "false"),
            package_xml("dependency_pinned-3.0.0.3", "false", "true", "true"),
            package_xml("dependency_floating-4.0.0.4", "false", "true", "false"),
            package_xml("included_dependency-5.0.0.5", "true", "true", "false"),
        ]
        .concat();
        let seed = read_xml("seed", &demo(&packages)).unwrap().seed();
        assert_eq!(
            seed.deps,
            [
                ("chosen_pinned".to_string(), "1.0.0.1".to_string()),
                ("chosen_floating".to_string(), ">=2.0.0.2".to_string()),
            ]
        );
        assert_eq!(seed.skipped, 3);
        assert_eq!(seed.source_labview, Some(2013));
        assert!(seed.dev_deps.is_empty() && seed.embedded.is_empty());
        assert!(seed.name.is_none() && seed.version.is_none() && seed.nipm.is_none());
    }

    #[test]
    fn a_name_listed_twice_keeps_the_last_and_counts_once() {
        let packages = [
            package_xml("demo_lib-1.0.0.1", "false", "false", "true"),
            package_xml("Demo_Lib-2.0.0.2", "false", "false", "false"),
        ]
        .concat();
        let seed = read_xml("twice", &demo(&packages)).unwrap().seed();
        assert_eq!(
            seed.deps,
            [("demo_lib".to_string(), ">=2.0.0.2".to_string())]
        );
        assert_eq!(seed.skipped, 0);
    }
}
