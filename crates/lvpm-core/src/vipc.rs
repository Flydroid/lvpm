//! Reading a VIPM `.vipc`: a zip whose `config.xml` lists the packages of one
//! LabVIEW target. The file is untrusted input, so the reader takes only what
//! it understands from the one `<Target>` and refuses everything else, with an
//! error that names the file. No XML dependency: the file is small and flat.

use crate::index::entry_from_bytes;
use crate::lvpkg::check_name;
use crate::ops::init::{Embedded, Seed};
use crate::project::same_name;
use crate::version::{Version, is_version};
use anyhow::{Result, anyhow};
use std::fs::File;
use std::io::{Read, Seek};
use std::path::Path;
use zip::ZipArchive;
use zip::result::ZipError;

/// The most of `config.xml` that is read; a real one is a few KiB.
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

/// The most of one embedded package archive that is read.
const MAX_EMBEDDED_BYTES: u64 = 256 * 1024 * 1024;

/// The most of all embedded archives together; they are held in memory until imported.
const MAX_EMBEDDED_TOTAL: u64 = 1024 * 1024 * 1024;

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

/// The zip at `path`; the error is what is wrong with the file.
fn open(path: &Path) -> Result<ZipArchive<File>, String> {
    let file = File::open(path).map_err(|e| format!("cannot be read: {e}"))?;
    ZipArchive::new(file).map_err(|e| match e {
        ZipError::Io(e) => format!("cannot be read: {e}"),
        _ => "not a zip file".to_string(),
    })
}

/// The text of the archive's `config.xml`; the error is what is wrong with the file.
fn read_config(path: &Path) -> Result<String, String> {
    let mut zip = open(path)?;
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
        // An unclosed `<Package>` would otherwise swallow the next one.
        if block.contains("<Package>") {
            return Err("<Package> is not closed".to_string());
        }
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

impl Package {
    /// Init takes it: VIPM did not list it only because another package needs it.
    fn chosen(&self) -> bool {
        !self.dependency
    }
}

impl Vipc {
    /// Each name once, as its last listing, in the order of those last listings.
    fn listings(&self) -> impl Iterator<Item = &Package> {
        let all = &self.packages;
        all.iter()
            .enumerate()
            .filter(move |(at, p)| !all[at + 1..].iter().any(|q| same_name(&q.name, &p.name)))
            .map(|(_, p)| p)
    }

    /// What init takes from it. A `Dependency` package is skipped and counted;
    /// a `Pinned` one gets its exact version and the rest a floor. A name listed
    /// twice follows its last listing and counts once.
    pub fn seed(&self) -> Seed {
        let mut seed = Seed {
            source_labview: Some(self.labview_year),
            ..Seed::default()
        };
        for package in self.listings() {
            if !package.chosen() {
                seed.skipped += 1;
                continue;
            }
            let constraint = match package.pinned {
                true => package.version.clone(),
                false => format!(">={}", package.version),
            };
            seed.deps.push((package.name.clone(), constraint));
        }
        seed
    }

    /// Add to `seed` the package archives the `.vipc` at `path` holds for the
    /// packages init adds that it marks `Included`. A `Dependency` one is left
    /// to the feeds: an import shadows every feed's versions of that name in
    /// the shared cache, so only what lvpm.toml lists may be planted. A package
    /// init adds and marks `Included` with no archive inside goes to
    /// `not_embedded`; an archive that cannot be taken is an error, and `seed`
    /// is then not to be used.
    pub fn embed(&self, path: &Path, label: &str, seed: &mut Seed) -> Result<()> {
        self.embed_within(path, label, seed, MAX_EMBEDDED_TOTAL)
    }

    /// `embed`, refusing archives that together hold more than `total` bytes.
    fn embed_within(&self, path: &Path, label: &str, seed: &mut Seed, total: u64) -> Result<()> {
        let held: Vec<&Package> = self
            .listings()
            .filter(|p| p.included && p.chosen())
            .collect();
        if held.is_empty() {
            return Ok(());
        }
        let mut zip = open(path).map_err(|what| anyhow!("{label}: {what}"))?;
        let mut used = 0;
        for package in held {
            // The total is held before reading, so no more than it is ever in memory.
            let room = total - used;
            let (cap, limit) = match room < MAX_EMBEDDED_BYTES {
                true => (
                    room,
                    format!("the {room} bytes left of {total} for all of them"),
                ),
                false => (MAX_EMBEDDED_BYTES, format!("{MAX_EMBEDDED_BYTES} bytes")),
            };
            let found = find_archive(&mut zip, package, cap, &limit)
                .map_err(|what| anyhow!("{label}: {what}"))?;
            let (name, version) = (package.name.clone(), package.version.clone());
            match found {
                Some((entry, bytes)) => {
                    used += bytes.len() as u64;
                    seed.embedded.push(Embedded {
                        name,
                        version,
                        // The resolved path, so two `p.vipc` in different folders stay apart in the cache.
                        label: format!("{}!/{entry}", path.display()),
                        bytes,
                    });
                }
                None => seed.not_embedded.push((name, version)),
            }
        }
        Ok(())
    }
}

/// `package`'s `<name>-<version>.vip` in `zip`, at any depth: its entry name
/// and its bytes, read up to `cap` (`limit` names it in the error). `None` when
/// no entry has that file name; the error is what is wrong with the archive,
/// including a spec that names another package or version than the listing.
/// The entry name is only looked at, never used as a path, and one with a
/// control character never matches.
fn find_archive<R: Read + Seek>(
    zip: &mut ZipArchive<R>,
    package: &Package,
    cap: u64,
    limit: &str,
) -> Result<Option<(String, Vec<u8>)>, String> {
    let wanted = format!("{}-{}.vip", package.name, package.version);
    let mut found = Vec::new();
    for at in 0..zip.len() {
        let entry = zip.by_index_raw(at).map_err(|e| e.to_string())?;
        let name = entry.name();
        if name.rsplit(['/', '\\']).next() == Some(wanted.as_str())
            && !name.chars().any(char::is_control)
        {
            found.push((at, name.to_string()));
        }
    }
    let (at, entry) = match found.pop() {
        None => return Ok(None),
        Some(only) if found.is_empty() => only,
        Some(_) => return Err(format!("more than one {wanted} inside")),
    };
    let not_a_package = |why: String| format!("embedded {entry} is not a package: {why}");
    let mut bytes = Vec::new();
    zip.by_index(at)
        .map_err(|e| not_a_package(e.to_string()))?
        .take(cap + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| not_a_package(e.to_string()))?;
    if bytes.len() as u64 > cap {
        return Err(format!("embedded {entry} is larger than {limit}"));
    }
    let spec =
        entry_from_bytes(Path::new(&entry), &bytes).map_err(|e| not_a_package(format!("{e:#}")))?;
    // The cache files it under the spec's name for every project, so it must be what was listed.
    if !same_name(&spec.name, &package.name) || spec.version != Version::parse(&package.version) {
        return Err(format!(
            "embedded {entry} is {} {}, not {} {}",
            spec.name, spec.version, package.name, package.version
        ));
    }
    Ok(Some((entry, bytes)))
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

    /// A config.xml for LabVIEW 2013 holding `packages`.
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
            [("Demo_Lib".to_string(), ">=2.0.0.2".to_string())]
        );
        assert_eq!(seed.skipped, 0);
    }

    #[test]
    fn a_name_whose_last_listing_is_a_dependency_is_skipped_and_not_embedded() {
        let dir = Dir::new("embed-last-dependency");
        let vip = vip_bytes("demo_lib", "1.0.0");
        let packages = [
            package_xml("demo_lib-1.0.0", "true", "false", "false"),
            package_xml("demo_lib-1.0.0", "true", "true", "false"),
        ]
        .concat();
        let path = dir.vipc(&[
            ("config.xml", demo(&packages).as_bytes()),
            ("demo_lib-1.0.0.vip", &vip),
        ]);
        let vipc = read(&path, "p.vipc").unwrap();
        let mut seed = vipc.seed();

        vipc.embed(&path, "p.vipc", &mut seed).unwrap();

        assert_eq!(
            (
                seed.deps.len(),
                seed.skipped,
                seed.embedded.len(),
                seed.not_embedded.len()
            ),
            (0, 1, 0, 0),
            "the last listing wins for the constraint and the import alike"
        );
    }

    /// A package archive: a zip whose `spec` names `name` at `version`.
    fn vip_bytes(name: &str, version: &str) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        zip.start_file("spec", SimpleFileOptions::default())
            .unwrap();
        write!(zip, "[Package]\nName={name}\nVersion={version}\n").unwrap();
        zip.finish().unwrap().into_inner()
    }

    fn demo_package() -> Package {
        Package {
            name: "demo_lib".into(),
            version: "1.0.0".into(),
            included: true,
            dependency: false,
            pinned: false,
        }
    }

    type Found = Result<Option<(String, Vec<u8>)>, String>;

    /// `find_archive` for `demo_lib-1.0.0` in a zip of `entries`, read up to `cap`.
    fn found_in(entries: &[(&str, &[u8])], cap: u64) -> Found {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, bytes) in entries {
            zip.start_file(*name, SimpleFileOptions::default()).unwrap();
            zip.write_all(bytes).unwrap();
        }
        let mut zip = ZipArchive::new(zip.finish().unwrap()).unwrap();
        find_archive(&mut zip, &demo_package(), cap, &format!("{cap} bytes"))
    }

    const CAP: u64 = 1 << 20;

    #[test]
    fn an_archive_is_found_by_its_file_name_at_any_depth() {
        let vip = vip_bytes("demo_lib", "1.0.0");
        for entry in [
            "demo_lib-1.0.0.vip",
            "Packages/demo_lib-1.0.0.vip",
            "a/b/c/demo_lib-1.0.0.vip",
            "Packages\\demo_lib-1.0.0.vip",
        ] {
            let found = found_in(&[("config.xml", b"x"), (entry, &vip)], CAP);

            assert_eq!(found, Ok(Some((entry.to_string(), vip.clone()))), "{entry}");
        }
    }

    #[test]
    fn an_entry_that_only_ends_alike_or_has_a_control_character_is_not_the_archive() {
        let vip = vip_bytes("demo_lib", "1.0.0");
        for entry in [
            "xdemo_lib-1.0.0.vip",
            "demo_lib-1.0.0.vip.bak",
            "demo_lib-1.0.1.vip",
            "demo_lib-1.0.0.vip/",
            "\u{1b}[31m/demo_lib-1.0.0.vip",
        ] {
            assert_eq!(found_in(&[(entry, &vip)], CAP), Ok(None), "{entry:?}");
        }
        assert_eq!(found_in(&[], CAP), Ok(None));
    }

    #[test]
    fn two_entries_with_the_archive_name_are_refused() {
        let vip = vip_bytes("demo_lib", "1.0.0");

        let why = found_in(
            &[
                ("a/demo_lib-1.0.0.vip", &vip),
                ("b/demo_lib-1.0.0.vip", &vip),
            ],
            CAP,
        );

        assert_eq!(why, Err("more than one demo_lib-1.0.0.vip inside".into()));
    }

    #[test]
    fn an_entry_of_exactly_the_cap_is_read_and_one_byte_over_is_refused() {
        let vip = vip_bytes("demo_lib", "1.0.0");
        let len = vip.len() as u64;

        let at_cap = found_in(&[("demo_lib-1.0.0.vip", &vip)], len);
        let over = found_in(&[("demo_lib-1.0.0.vip", &vip)], len - 1);

        assert_eq!(at_cap.unwrap().map(|(_, bytes)| bytes), Some(vip));
        assert_eq!(
            over,
            Err(format!(
                "embedded demo_lib-1.0.0.vip is larger than {} bytes",
                len - 1
            ))
        );
    }

    #[test]
    fn an_entry_that_is_not_a_package_is_refused_with_the_reason() {
        let why = found_in(&[("Packages/demo_lib-1.0.0.vip", b"not a zip")], CAP).unwrap_err();

        assert!(
            why.starts_with("embedded Packages/demo_lib-1.0.0.vip is not a package: "),
            "{why}"
        );
    }

    #[test]
    fn embed_takes_the_included_archives_init_adds_and_names_where_each_came_from() {
        let dir = Dir::new("embed");
        let vip = vip_bytes("demo_lib", "1.0.0");
        let needed = vip_bytes("needed_lib", "4.0.0");
        let packages = [
            package_xml("demo_lib-1.0.0", "true", "false", "false"),
            package_xml("absent_lib-2.0.0", "true", "false", "false"),
            package_xml("plain_lib-3.0.0", "false", "false", "false"),
            package_xml("needed_lib-4.0.0", "true", "true", "false"),
            package_xml("ghost_lib-5.0.0", "true", "true", "false"),
        ]
        .concat();
        let path = dir.vipc(&[
            ("config.xml", demo(&packages).as_bytes()),
            ("Packages/demo_lib-1.0.0.vip", &vip),
            ("Packages/needed_lib-4.0.0.vip", &needed),
        ]);
        let vipc = read(&path, "p.vipc").unwrap();
        let mut seed = vipc.seed();

        vipc.embed(&path, "p.vipc", &mut seed).unwrap();

        assert_eq!(
            seed.embedded.len(),
            1,
            "a Dependency is left to the feeds, though its archive is inside"
        );
        let embedded = &seed.embedded[0];
        assert_eq!(
            (
                &embedded.name[..],
                &embedded.version[..],
                &embedded.label[..]
            ),
            (
                "demo_lib",
                "1.0.0",
                &format!("{}!/Packages/demo_lib-1.0.0.vip", path.display())[..]
            )
        );
        assert_eq!(embedded.bytes, vip);
        assert_eq!(
            seed.not_embedded,
            [("absent_lib".to_string(), "2.0.0".to_string())]
        );
    }

    #[test]
    fn embed_refuses_a_bad_archive_naming_the_file_as_typed() {
        let dir = Dir::new("embed-bad");
        let packages = package_xml("demo_lib-1.0.0", "true", "false", "false");
        let path = dir.vipc(&[
            ("config.xml", demo(&packages).as_bytes()),
            ("demo_lib-1.0.0.vip", b"junk"),
        ]);
        let vipc = read(&path, "as/typed.vipc").unwrap();

        let why = vipc
            .embed(&path, "as/typed.vipc", &mut vipc.seed())
            .unwrap_err()
            .to_string();

        assert!(
            why.starts_with("as/typed.vipc: embedded demo_lib-1.0.0.vip is not a package: "),
            "{why}"
        );
    }

    #[test]
    fn a_vipc_that_embeds_nothing_is_not_opened_again() {
        let dir = Dir::new("embed-none");
        let path = dir.config(&demo(&chosen("demo_lib-1.0.0")));
        let vipc = read(&path, "p.vipc").unwrap();
        std::fs::remove_file(&path).unwrap();
        let mut seed = vipc.seed();

        vipc.embed(&path, "p.vipc", &mut seed).unwrap();

        assert!(seed.embedded.is_empty() && seed.not_embedded.is_empty());
    }

    #[test]
    fn a_name_listed_twice_is_embedded_once_and_the_total_is_capped() {
        let dir = Dir::new("embed-twice");
        let vip = vip_bytes("demo_lib", "1.0.0");
        let packages = [
            package_xml("demo_lib-1.0.0", "true", "false", "false"),
            package_xml("demo_lib-1.0.0", "true", "false", "false"),
        ]
        .concat();
        let path = dir.vipc(&[
            ("config.xml", demo(&packages).as_bytes()),
            ("demo_lib-1.0.0.vip", &vip),
        ]);
        let vipc = read(&path, "p.vipc").unwrap();
        let (mut seed, mut small) = (vipc.seed(), vipc.seed());

        vipc.embed(&path, "p.vipc", &mut seed).unwrap();
        let err = vipc
            .embed_within(&path, "p.vipc", &mut small, vip.len() as u64 - 1)
            .unwrap_err()
            .to_string();

        assert_eq!(seed.embedded.len(), 1);
        assert_eq!(
            err,
            format!(
                "p.vipc: embedded demo_lib-1.0.0.vip is larger than the {0} bytes left of {0} for all of them",
                vip.len() - 1
            )
        );
    }

    #[test]
    fn an_archive_whose_spec_names_another_package_or_version_is_refused() {
        for (name, version) in [("other_lib", "1.0.0"), ("demo_lib", "99.0.0")] {
            let vip = vip_bytes(name, version);

            let why = found_in(&[("demo_lib-1.0.0.vip", &vip)], CAP);

            assert_eq!(
                why,
                Err(format!(
                    "embedded demo_lib-1.0.0.vip is {name} {version}, not demo_lib 1.0.0"
                ))
            );
        }
    }
}
