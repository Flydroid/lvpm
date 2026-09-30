//! `lvpm package build`: build a package from the `[package]` table of an `lvpm.toml`,
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
//! icon = "icon.bmp"                     # optional BMP; default: a plain 36x36 icon
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
use std::io::{Cursor, Write};
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
    /// Package icon (BMP); a plain default icon if not set.
    pub icon: Option<PathBuf>,
}

/// A plain 36×36 24-bit BMP, the size and format of VIPM's own icons: a grey
/// tile with a dark border.
pub fn default_icon() -> Vec<u8> {
    const SIZE: u32 = 36;
    let row = (SIZE * 3).div_ceil(4) * 4;
    let data_len = row * SIZE;
    let mut bmp = Vec::with_capacity(54 + data_len as usize);
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&(54 + data_len).to_le_bytes()); // file size
    bmp.extend_from_slice(&0u32.to_le_bytes()); // reserved
    bmp.extend_from_slice(&54u32.to_le_bytes()); // pixel data offset
    bmp.extend_from_slice(&40u32.to_le_bytes()); // BITMAPINFOHEADER
    bmp.extend_from_slice(&(SIZE as i32).to_le_bytes());
    bmp.extend_from_slice(&(SIZE as i32).to_le_bytes());
    bmp.extend_from_slice(&1u16.to_le_bytes()); // planes
    bmp.extend_from_slice(&24u16.to_le_bytes()); // bits per pixel
    bmp.extend_from_slice(&0u32.to_le_bytes()); // no compression
    bmp.extend_from_slice(&data_len.to_le_bytes());
    bmp.extend_from_slice(&[0u8; 16]); // resolution, palette: unused
    for y in 0..SIZE {
        for x in 0..SIZE {
            let border = x < 2 || y < 2 || x >= SIZE - 2 || y >= SIZE - 2;
            bmp.extend_from_slice(if border { &[0x40, 0x40, 0x40] } else { &[0xD0, 0xD0, 0xD0] });
        }
        bmp.resize(bmp.len() + (row - SIZE * 3) as usize, 0);
    }
    bmp
}

/// Zero the external file attributes of every central-directory entry, as
/// VIPM writes them. The `zip` crate always stores a Unix file mode there.
fn clear_external_attributes(bytes: &mut [u8]) -> Result<()> {
    let u16_at = |b: &[u8], i: usize| u16::from_le_bytes([b[i], b[i + 1]]) as usize;
    let u32_at = |b: &[u8], i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as usize;
    let eocd = (0..=bytes.len().saturating_sub(22))
        .rev()
        .find(|&i| bytes[i..].starts_with(b"PK\x05\x06"))
        .context("no end of central directory")?;
    let entries = u16_at(bytes, eocd + 10);
    let mut i = u32_at(bytes, eocd + 16);
    for _ in 0..entries {
        ensure!(bytes[i..].starts_with(b"PK\x01\x02"), "corrupt central directory");
        bytes[i + 38..i + 42].fill(0);
        i += 46 + u16_at(bytes, i + 28) + u16_at(bytes, i + 30) + u16_at(bytes, i + 32);
    }
    Ok(())
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
    icon: Option<String>,
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
        icon: t.icon.map(|icon| base_dir.join(icon)),
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

    /// Write the package as VIPM lays it out: `icon.bmp` first, the files,
    /// `spec` last, and no file attributes in the central directory.
    pub fn write<W: Write>(&self, mut out: W) -> Result<W> {
        let spec = self.spec();
        ensure!(
            spec.is_ascii(),
            "package metadata and file names must be ASCII (the spec is)"
        );
        let icon = match &self.icon {
            Some(path) => {
                let bmp = std::fs::read(path).with_context(|| format!("reading icon {}", path.display()))?;
                ensure!(bmp.starts_with(b"BM"), "icon {} is not a BMP file", path.display());
                bmp
            }
            None => default_icon(),
        };
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("icon.bmp", options)?;
        zip.write_all(&icon)?;
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
        let mut bytes = zip.finish()?.into_inner();
        clear_external_attributes(&mut bytes)?;
        out.write_all(&bytes)?;
        Ok(out)
    }

    /// Write `<output>/<name>-<version>.vip` (replacing one of that name);
    /// returns its path.
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
    fn layout_matches_vipm_icon_first_and_no_file_attributes() {
        let dir = sources();
        let p = parse(MANIFEST, dir.path()).unwrap();
        let bytes = p.write(Vec::new()).unwrap();

        let mut zip = zip::ZipArchive::new(Cursor::new(bytes.clone())).unwrap();
        assert_eq!(zip.by_index(0).unwrap().name(), "icon.bmp");
        let mut icon = Vec::new();
        zip.by_index(0).unwrap().read_to_end(&mut icon).unwrap();
        assert_eq!(icon, default_icon());

        // Every central directory entry: external attributes 0, as VIPM writes.
        let cd = bytes.windows(4).enumerate().filter(|(_, w)| *w == b"PK\x01\x02").map(|(i, _)| i).collect::<Vec<_>>();
        assert_eq!(cd.len(), zip.len());
        for i in cd {
            assert_eq!(&bytes[i + 38..i + 42], &[0, 0, 0, 0], "attributes at {i}");
        }
    }

    #[test]
    fn default_icon_is_a_36x36_24bit_bmp() {
        let bmp = default_icon();
        let u32_at = |i: usize| u32::from_le_bytes(bmp[i..i + 4].try_into().unwrap());
        assert_eq!(&bmp[..2], b"BM");
        assert_eq!(u32_at(2) as usize, bmp.len());
        assert_eq!((u32_at(18), u32_at(22)), (36, 36));
        assert_eq!(u16::from_le_bytes([bmp[28], bmp[29]]), 24);
        assert_eq!(bmp.len(), 54 + 36 * 108);
    }

    #[test]
    fn custom_icon_is_used_and_must_be_a_bmp() {
        let dir = sources();
        std::fs::write(dir.path().join("icon.bmp"), b"BMcustom").unwrap();
        std::fs::write(dir.path().join("icon.png"), b"\x89PNG").unwrap();
        let with = |icon: &str| MANIFEST.replace("labview = \"25.0\"", &format!("labview = \"25.0\"\nicon = \"{icon}\""));

        let p = parse(&with("icon.bmp"), dir.path()).unwrap();
        let bytes = p.write(Vec::new()).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut icon = Vec::new();
        zip.by_name("icon.bmp").unwrap().read_to_end(&mut icon).unwrap();
        assert_eq!(icon, b"BMcustom");

        let p = parse(&with("icon.png"), dir.path()).unwrap();
        assert!(format!("{:#}", p.write(Vec::new()).unwrap_err()).contains("not a BMP"));
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
