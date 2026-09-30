//! `lvpm pack`: build a package from the `[package]` table of an `lvpm.toml`,
//! without LabVIEW or VIPM.
//!
//! The output is the vip 2017 format VIPM writes and `install` reads: a ZIP
//! with the files under `File Group N/` and an INI `spec` (CRLF, ASCII,
//! written last) saying where each group goes. Everything lands in one group
//! with `Target Dir="<application>"`, i.e. paths are relative to the LabVIEW
//! directory — the same tree an LVAddons addon mirrors under `<pkg>/1/`.
//!
//! ```toml
//! [package]
//! name = "nfg_lib_adlink_pxi_trigger"
//! version = "0.3.0.0"
//! display-name = "ADLINK PXI Trigger"
//! vendor = "NFG"
//! labview = "25.0"                      # minimum LabVIEW version
//! output = "dist"                       # relative to lvpm.toml, default "dist"
//!
//! [package.requires]                    # optional, same syntax as [dependencies]
//! oglib_error = ">=6.0.1"
//!
//! [[package.files]]
//! source = "LabVIEW"                    # a folder (recursively) or a single file
//! target = "vi.lib/NFG/ADLINK PXI Trigger"
//! exclude = ["Report.html", "*.dll"]    # relative to source; `*` matches within a name
//! ```

use crate::index::md5_hex;
use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use std::io::{Seek, Write};
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

/// What `[package]` describes.
#[derive(Debug, Clone)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub display_name: String,
    pub vendor: String,
    pub summary: String,
    pub description: String,
    pub release_notes: String,
    /// Minimum LabVIEW version, e.g. `25.0`.
    pub labview: String,
    /// `[Dependencies] Requires`, e.g. `oglib_error>=6.0.1`.
    pub requires: Vec<String>,
    /// (path below the LabVIEW directory with `/`, file on disk), sorted.
    pub files: Vec<(String, PathBuf)>,
    pub output: PathBuf,
}

#[derive(Deserialize)]
struct File {
    package: Option<PackageTable>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct PackageTable {
    name: String,
    version: String,
    display_name: Option<String>,
    #[serde(default)]
    vendor: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    release_notes: String,
    labview: String,
    output: Option<String>,
    #[serde(default)]
    requires: toml::Table,
    #[serde(default)]
    files: Vec<FilesEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FilesEntry {
    source: String,
    target: String,
    #[serde(default)]
    exclude: Vec<String>,
}

/// Parse the `[package]` table of `text`; paths resolve against `base_dir`.
pub fn parse(text: &str, base_dir: &Path) -> Result<Package> {
    let file: File = toml::from_str(text)?;
    let t = file.package.context("no [package] table")?;

    ensure!(
        !t.name.is_empty()
            && t.name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)),
        "[package] name {:?} must be ASCII letters, digits, `.`, `_` or `-`",
        t.name
    );
    check_version(&t.version)?;
    ensure!(!t.files.is_empty(), "[package] lists no files");

    let mut requires = Vec::new();
    for (name, value) in &t.requires {
        let toml::Value::String(constraint) = value else {
            bail!("[package.requires] {name}: expected a string like \">=1.0\"");
        };
        requires.push(requirement(name, constraint));
    }

    let mut files = Vec::new();
    for entry in &t.files {
        files.extend(collect(base_dir, entry)?);
    }
    files.sort();
    if let Some(w) = files
        .windows(2)
        .find(|w| w[0].0.eq_ignore_ascii_case(&w[1].0))
    {
        bail!(
            "{} is packaged twice (from {} and {}); exclude one of them",
            w[0].0,
            w[0].1.display(),
            w[1].1.display()
        );
    }

    Ok(Package {
        display_name: t.display_name.unwrap_or_else(|| t.name.clone()),
        name: t.name,
        version: t.version,
        vendor: t.vendor,
        summary: t.summary,
        description: t.description,
        release_notes: t.release_notes,
        labview: t.labview,
        requires,
        files,
        output: base_dir.join(t.output.as_deref().unwrap_or("dist")),
    })
}

/// Up to four dot-separated numbers, as VIPM versions are.
pub fn check_version(version: &str) -> Result<()> {
    let parts: Vec<&str> = version.split('.').collect();
    ensure!(
        (1..=4).contains(&parts.len())
            && parts
                .iter()
                .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())),
        "version {version:?} must be up to four dot-separated numbers, e.g. 1.2.3.4"
    );
    Ok(())
}

/// A `[dependencies]`-style constraint as `Requires` syntax.
fn requirement(name: &str, constraint: &str) -> String {
    let c = constraint.trim();
    match c {
        "" | "*" => name.to_string(),
        _ if c.starts_with(['>', '<', '=']) => format!("{name}{c}"),
        _ => format!("{name}={c}"),
    }
}

/// `*` matches any run of characters except `/`; everything else literally.
fn glob_match(pattern: &str, text: &str) -> bool {
    fn go(p: &[u8], t: &[u8]) -> bool {
        match (p.first(), t.first()) {
            (None, None) => true,
            (Some(b'*'), _) => go(&p[1..], t) || (!t.is_empty() && t[0] != b'/' && go(p, &t[1..])),
            (Some(a), Some(b)) if a.eq_ignore_ascii_case(b) => go(&p[1..], &t[1..]),
            _ => false,
        }
    }
    go(pattern.as_bytes(), text.as_bytes())
}

fn collect(base_dir: &Path, entry: &FilesEntry) -> Result<Vec<(String, PathBuf)>> {
    let source = base_dir.join(&entry.source);
    let target = entry.target.trim_matches('/').replace('\\', "/");
    ensure!(
        !target.split('/').any(|part| part == ".."),
        "target {:?} must stay inside the LabVIEW directory",
        entry.target
    );
    let join = |rel: &str| {
        if target.is_empty() {
            rel.to_string()
        } else {
            format!("{target}/{rel}")
        }
    };

    if source.is_file() {
        let name = source
            .file_name()
            .context("file without a name")?
            .to_string_lossy()
            .into_owned();
        return Ok(vec![(join(&name), source)]);
    }
    ensure!(source.is_dir(), "{} does not exist", source.display());

    let mut found = Vec::new();
    walk(&source, "", &mut found)?;
    Ok(found
        .into_iter()
        .filter(|(rel, _)| !entry.exclude.iter().any(|pattern| glob_match(pattern, rel)))
        .map(|(rel, path)| (join(&rel), path))
        .collect())
}

fn walk(dir: &Path, rel: &str, out: &mut Vec<(String, PathBuf)>) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = if rel.is_empty() {
            name
        } else {
            format!("{rel}/{name}")
        };
        if entry.file_type()?.is_dir() {
            walk(&entry.path(), &rel, out)?;
        } else {
            out.push((rel, entry.path()));
        }
    }
    Ok(())
}

impl Package {
    /// `<name>-<version>.vip`
    pub fn file_name(&self) -> String {
        format!("{}-{}.vip", self.name, self.version)
    }

    /// Stable package ID: the MD5 of `name-version`.
    pub fn id(&self) -> String {
        md5_hex(format!("{}-{}", self.name, self.version).as_bytes())
    }

    /// The `spec`, laid out as VIPM writes it.
    pub fn spec(&self) -> String {
        let q = |v: &str| format!("\"{}\"", v.replace('"', "'").replace(['\r', '\n'], " "));
        let mut lines: Vec<String> = Vec::new();
        let mut section = |name: &str, entries: Vec<String>| {
            if !lines.is_empty() {
                lines.extend([String::new(), String::new()]);
            }
            lines.push(format!("[{name}]"));
            lines.extend(entries);
        };
        section(
            "Package",
            vec![
                format!("Name={}", q(&self.name)),
                format!("Version={}", q(&self.version)),
                "Release=\"\"".into(),
                format!("ID={}", self.id()),
                "File Format=\"vip\"".into(),
                "Format Version=\"2017\"".into(),
                format!("Display Name={}", q(&self.display_name)),
            ],
        );
        section(
            "Description",
            vec![
                format!("Description={}", q(&self.description)),
                format!("Summary={}", q(&self.summary)),
                "License=\"\"".into(),
                "Copyright=\"\"".into(),
                "Distribution=\"\"".into(),
                format!("Vendor={}", q(&self.vendor)),
                "URL=\"\"".into(),
                "Packager=\"\"".into(),
                "Demo=\"FALSE\"".into(),
                format!("Release Notes={}", q(&self.release_notes)),
                "System Package=\"FALSE\"".into(),
                "Sub Package=\"FALSE\"".into(),
                "License Agreement=\"FALSE\"".into(),
            ],
        );
        section(
            "LabVIEW",
            vec![
                "close labview before install=\"FALSE\"".into(),
                "restart labview after install=\"FALSE\"".into(),
                "skip mass compile after install=\"FALSE\"".into(),
                "install into global environment=\"FALSE\"".into(),
            ],
        );
        section(
            "Platform",
            vec![
                format!(
                    "Exclusive_LabVIEW_Version={}",
                    q(&format!("LabVIEW>={}", self.labview))
                ),
                "Exclusive_LabVIEW_System=\"ALL\"".into(),
                "Exclusive_OS=\"ALL\"".into(),
            ],
        );
        section(
            "Script VIs",
            [
                "PreInstall",
                "PostInstall",
                "PreUninstall",
                "PostUninstall",
                "Verify",
                "PreBuild",
                "PostBuild",
            ]
            .iter()
            .map(|h| format!("{h}=\"\""))
            .collect(),
        );
        section(
            "Dependencies",
            vec![
                "AutoReqProv=FALSE".into(),
                format!("Requires={}", q(&self.requires.join(","))),
                "Conflicts=\"\"".into(),
            ],
        );
        section(
            "Activation",
            vec!["License File=\"\"".into(), "Licensed Library=\"\"".into()],
        );
        section(
            "Files",
            vec![
                "Num File Groups=\"1\"".into(),
                "Sub-Packages=\"\"".into(),
                "Namespaces=\"\"".into(),
            ],
        );
        let mut group = vec![
            "Target Dir=\"<application>\"".into(),
            "Replace Mode=\"Always\"".into(),
            format!("Num Files={}", self.files.len()),
        ];
        group.extend(
            self.files
                .iter()
                .enumerate()
                .map(|(i, (path, _))| format!("File {i}={}", q(path))),
        );
        section("File Group 0", group);
        lines.push(String::new());
        lines.join("\r\n")
    }

    /// Write the package: the files, then `spec` last (as VIPM does).
    pub fn write<W: Write + Seek>(&self, out: W) -> Result<W> {
        let spec = self.spec();
        ensure!(
            spec.is_ascii(),
            "package metadata and file names must be ASCII (the spec is)"
        );
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        let mut zip = ZipWriter::new(out);
        for (path, source) in &self.files {
            zip.start_file(format!("File Group 0/{path}"), options)?;
            std::io::copy(
                &mut std::fs::File::open(source)
                    .with_context(|| format!("reading {}", source.display()))?,
                &mut zip,
            )?;
        }
        zip.start_file("spec", options)?;
        zip.write_all(spec.as_bytes())?;
        Ok(zip.finish()?)
    }

    /// Write `<output>/<name>-<version>.vip`; returns its path.
    pub fn build(&self) -> Result<PathBuf> {
        std::fs::create_dir_all(&self.output)
            .with_context(|| format!("creating {}", self.output.display()))?;
        let path = self.output.join(self.file_name());
        let file =
            std::fs::File::create(&path).with_context(|| format!("creating {}", path.display()))?;
        self.write(file)?;
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Read};

    fn tree(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (rel, content) in files {
            let p = dir.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, content).unwrap();
        }
        dir
    }

    const MANIFEST: &str = r#"
[project]
name = "adlink"

[package]
name = "nfg_lib_adlink_pxi_trigger"
version = "0.3.0.1"
display-name = "ADLINK PXI Trigger"
vendor = "NFG"
summary = "Trigger routing"
labview = "25.0"

[package.requires]
oglib_error = ">=6.0.1"
jki_lib_state_machine = "*"
exact_one = "1.2.3.4"

[[package.files]]
source = "LabVIEW"
target = "vi.lib/NFG/ADLINK PXI Trigger"
exclude = ["Report.html", "*.dll"]

[[package.files]]
source = "dist/wrapper64.dll"
target = "vi.lib/NFG/ADLINK PXI Trigger/"
"#;

    fn sources() -> tempfile::TempDir {
        tree(&[
            ("LabVIEW/lib.lvlib", "lvlib"),
            ("LabVIEW/Report.html", "report"),
            ("LabVIEW/old.dll", "old build"),
            ("LabVIEW/VIs/b.vi", "b"),
            ("LabVIEW/VIs/a.vi", "a"),
            ("dist/wrapper64.dll", "new build"),
        ])
    }

    #[test]
    fn parses_the_package_table() {
        let dir = sources();
        let p = parse(MANIFEST, dir.path()).unwrap();
        assert_eq!(p.file_name(), "nfg_lib_adlink_pxi_trigger-0.3.0.1.vip");
        assert_eq!(p.output, dir.path().join("dist"));
        assert_eq!(
            p.requires,
            [
                "oglib_error>=6.0.1",
                "jki_lib_state_machine",
                "exact_one=1.2.3.4"
            ]
        );
        let paths: Vec<&str> = p.files.iter().map(|(f, _)| f.as_str()).collect();
        assert_eq!(
            paths,
            [
                "vi.lib/NFG/ADLINK PXI Trigger/VIs/a.vi",
                "vi.lib/NFG/ADLINK PXI Trigger/VIs/b.vi",
                "vi.lib/NFG/ADLINK PXI Trigger/lib.lvlib",
                "vi.lib/NFG/ADLINK PXI Trigger/wrapper64.dll",
            ]
        );
    }

    #[test]
    fn written_package_reads_back_with_lvpms_own_parser() {
        let dir = sources();
        let p = parse(MANIFEST, dir.path()).unwrap();
        let bytes = p.write(Cursor::new(Vec::new())).unwrap().into_inner();

        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let names: Vec<String> = (0..zip.len())
            .map(|i| zip.by_index(i).unwrap().name().to_string())
            .collect();
        assert_eq!(names.last().unwrap(), "spec", "spec is written last");
        let mut dll = String::new();
        zip.by_name("File Group 0/vi.lib/NFG/ADLINK PXI Trigger/wrapper64.dll")
            .unwrap()
            .read_to_string(&mut dll)
            .unwrap();
        assert_eq!(dll, "new build");

        let mut text = String::new();
        zip.by_name("spec")
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert!(!text.replace("\r\n", "").contains('\n'), "CRLF only");
        let spec = crate::spec::parse(&text).unwrap();
        assert_eq!(spec.name, "nfg_lib_adlink_pxi_trigger");
        assert_eq!(spec.version, "0.3.0.1");
        assert_eq!(spec.display_name.as_deref(), Some("ADLINK PXI Trigger"));
        assert_eq!(spec.lv_gate.as_deref(), Some("LabVIEW>=25.0"));
        assert_eq!(
            spec.requires.as_deref(),
            Some("oglib_error>=6.0.1,jki_lib_state_machine,exact_one=1.2.3.4")
        );
        assert_eq!(spec.file_groups.len(), 1);
        assert_eq!(spec.file_groups[0].target_dir, "<application>");
        assert_eq!(spec.file_groups[0].files.len(), 4);
        assert!(
            spec.script_vis.iter().all(|(_, vi)| vi.is_empty()),
            "no hooks"
        );
    }

    #[test]
    fn id_is_stable_per_name_and_version() {
        let dir = sources();
        let p = parse(MANIFEST, dir.path()).unwrap();
        assert_eq!(p.id().len(), 32);
        assert_eq!(p.id(), parse(MANIFEST, dir.path()).unwrap().id());
        let q = parse(&MANIFEST.replace("0.3.0.1", "0.3.0.2"), dir.path()).unwrap();
        assert_ne!(p.id(), q.id());
    }

    #[test]
    fn a_file_packaged_twice_is_an_error() {
        let dir = sources();
        let manifest = MANIFEST.replace(r#"exclude = ["Report.html", "*.dll"]"#, "");
        let dir2 = tree(&[("LabVIEW/wrapper64.dll", "x"), ("dist/wrapper64.dll", "y")]);
        let _ = dir;
        let err = parse(&manifest, dir2.path()).unwrap_err().to_string();
        assert!(err.contains("packaged twice"), "{err}");
    }

    #[test]
    fn rejects_bad_input() {
        let dir = sources();
        for (from, to, needle) in [
            ("0.3.0.1", "0.3.x", "version"),
            ("0.3.0.1", "1.2.3.4.5", "version"),
            ("nfg_lib_adlink_pxi_trigger", "bad name", "name"),
            (
                "vi.lib/NFG/ADLINK PXI Trigger\"\nexclude",
                "../outside\"\nexclude",
                "inside the LabVIEW directory",
            ),
            (
                "source = \"LabVIEW\"",
                "source = \"Missing\"",
                "does not exist",
            ),
            ("summary =", "unknown-key = 1\nsummary =", "unknown field"),
        ] {
            let err = format!(
                "{:#}",
                parse(&MANIFEST.replace(from, to), dir.path()).unwrap_err()
            );
            assert!(err.contains(needle), "{to}: {err}");
        }
        assert!(
            parse("[project]\nname = \"x\"\n", dir.path()).is_err(),
            "no [package]"
        );
    }

    #[test]
    fn glob_matches_within_a_name() {
        assert!(glob_match("*.dll", "old.dll"));
        assert!(glob_match("*.DLL", "old.dll"));
        assert!(
            !glob_match("*.dll", "sub/old.dll"),
            "* doesn't cross folders"
        );
        assert!(glob_match("sub/*.dll", "sub/old.dll"));
        assert!(glob_match("Report.html", "Report.html"));
        assert!(!glob_match("Report.html", "VIs/Report.html"));
    }

    #[test]
    fn non_ascii_metadata_is_refused() {
        let dir = sources();
        let p = parse(
            &MANIFEST.replace("Trigger routing", "Trigger für Chassis"),
            dir.path(),
        )
        .unwrap();
        assert!(p.write(Cursor::new(Vec::new())).is_err());
    }
}
