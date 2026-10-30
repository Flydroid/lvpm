//! The `.lvpkg`: lvpm's own package format (ADR 6).
//!
//! A deterministic zip of `RECORD`, `package.toml` and the payload, the
//! library's members at `<install-dir>/<relative path>`, in the layout of the
//! LabVIEW tree. `package.toml` carries the package's metadata with
//! `lvpm.toml`'s table and key names; `RECORD` lists every other entry with its
//! SHA-256, one `<sha256 hex>  <path>` line each.

use crate::project::{Constraint, labview_minimum};
use crate::spec::{FileGroup, Spec};
use anyhow::{Context, Result, bail};
use serde::{Serialize, Serializer};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read, Seek, Write};
use zip::ZipArchive;
use zip::write::SimpleFileOptions;

/// The entry that lists every other entry with its SHA-256.
pub const RECORD: &str = "RECORD";
/// The entry that carries the package's metadata.
pub const PACKAGE_TOML: &str = "package.toml";

/// The most of `RECORD` or `package.toml` that is read: real ones are a few
/// KiB, and an archive from a feed must not inflate one until memory runs out.
const MAX_META_BYTES: u64 = 1024 * 1024;
/// The most of any other entry that is read. A read is streamed, so this
/// bounds time: a Deflate bomb stops here.
pub(crate) const MAX_ENTRY_BYTES: u64 = 1024 * 1024 * 1024;
/// The most that all the entries of one archive may unpack to.
const MAX_TOTAL_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// What every message about an archive that disagrees with its `RECORD` ends in.
const ALTERED: &str = "(the package was altered or is corrupt); rebuild or download it again";

/// What `package.toml` says about a package.
#[derive(Debug, Clone, PartialEq)]
pub struct Meta {
    pub name: String,
    /// As `lvpm.toml` has it: three parts stay three.
    pub version: String,
    /// The oldest LabVIEW, as `lvpm.toml` writes it: `">=2016"`.
    pub labview: String,
    pub vendor: String,
    pub display_name: String,
    /// Where the payload goes, below the LabVIEW folder, with forward slashes.
    pub install_dir: String,
    /// `[dependencies]` as `lvpm.toml` has them, in file order: name, constraint.
    pub dependencies: Vec<(String, String)>,
}

/// `package.toml`'s shape. The field order is the file's order: serde writes
/// the plain `format` first, then each table.
#[derive(Serialize)]
struct Wire<'a> {
    format: u32,
    project: WireProject<'a>,
    distribution: WireDistribution<'a>,
    #[serde(serialize_with = "as_table")]
    dependencies: &'a [(String, String)],
}

#[derive(Serialize)]
struct WireProject<'a> {
    name: &'a str,
    version: &'a str,
    labview: &'a str,
}

#[derive(Serialize)]
struct WireDistribution<'a> {
    kind: &'a str,
    vendor: &'a str,
    #[serde(rename = "display-name")]
    display_name: &'a str,
    #[serde(rename = "install-dir")]
    install_dir: &'a str,
}

fn as_table<S: Serializer>(deps: &&[(String, String)], s: S) -> Result<S::Ok, S::Error> {
    s.collect_map(deps.iter().map(|(name, constraint)| (name, constraint)))
}

impl Meta {
    /// `package.toml`'s text.
    pub fn to_toml(&self) -> String {
        toml::to_string(&Wire {
            format: 1,
            project: WireProject {
                name: &self.name,
                version: &self.version,
                labview: &self.labview,
            },
            distribution: WireDistribution {
                kind: "source",
                vendor: &self.vendor,
                display_name: &self.display_name,
                install_dir: &self.install_dir,
            },
            dependencies: &self.dependencies,
        })
        .expect("strings and string tables serialise")
    }
}

/// The package as bytes: `RECORD`, `package.toml`, then `files` (path below
/// `install-dir`, content) at `<install-dir>/<path>`, sorted byte-wise by path.
///
/// Nothing in an entry varies between two writes of the same input: fixed 1980
/// timestamps, one Deflate level, the zip crate's default permissions and no
/// extra fields.
///
/// Fails, as [`read`] would, on an `install-dir` or a member path that may not
/// go into a package, and on two members that are one file on Windows. A
/// member of 4 GiB or more fails too: a zip without zip64 cannot hold it.
pub fn write(meta: &Meta, files: &[(String, Vec<u8>)]) -> Result<Vec<u8>> {
    check_install_dir(&meta.install_dir)?;
    let mut lower: HashMap<String, &str> = HashMap::new();
    for (rel, _) in files {
        if !clean_path(rel) {
            bail!("member {rel:?} cannot go into a package: {BAD_PATH}");
        }
        match lower.insert(rel.to_lowercase(), rel) {
            Some(first) if first == rel => bail!("member {rel:?} is listed twice"),
            Some(first) => bail!("members {first:?} and {rel:?} differ only in letter case"),
            None => {}
        }
    }
    let toml = meta.to_toml();
    let mut payload: Vec<(String, &[u8])> = files
        .iter()
        .map(|(rel, bytes)| (format!("{}/{rel}", meta.install_dir), bytes.as_slice()))
        .collect();
    payload.sort_by(|a, b| a.0.cmp(&b.0));
    let mut entries = vec![(PACKAGE_TOML.to_string(), toml.as_bytes())];
    entries.extend(payload);

    let record: String = entries
        .iter()
        .map(|(path, bytes)| format!("{}  {path}\n", crate::cache::sha256_hex(bytes)))
        .collect();

    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .compression_level(Some(6))
        .last_modified_time(zip::DateTime::default());
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let mut put = |path: &str, bytes: &[u8]| -> Result<()> {
        zip.start_file(path, options)
            .and_then(|()| Ok(zip.write_all(bytes)?))
            .with_context(|| format!("writing entry {path:?}"))
    };
    put(RECORD, record.as_bytes())?;
    for (path, bytes) in &entries {
        put(path, bytes)?;
    }
    Ok(zip.finish()?.into_inner())
}

/// What may not go into a package, for messages.
const BAD_PATH: &str = "use a relative path with forward slashes, and no segment that is empty, \
     `.`, `..`, ends in a dot or a space, or names a Windows device (CON, NUL, COM1, ...)";

/// The LabVIEW gate of a package that needs LabVIEW `year` or later: `2016` is
/// `LabVIEW>=16.0`.
pub(crate) fn lv_gate(year: u32) -> String {
    format!("LabVIEW>={}.0", year - 2000)
}

/// A dependency as a `Requires` item. An exact version is written as a minimum,
/// like the published pilots do: a library pinning exact versions makes
/// consumers unsolvable. `*` is the bare name.
pub(crate) fn requires_item(dep: &str, c: &Constraint) -> String {
    match c {
        Constraint::Exact(v) | Constraint::AtLeast(v) => format!("{dep}>={}", v.raw),
        Constraint::Any => dep.to_string(),
    }
}

/// Whether `dir` may be a package's `install-dir`: a path below the LabVIEW
/// folder that [`clean_path`] accepts.
pub fn check_install_dir(dir: &str) -> Result<()> {
    if !clean_path(dir) {
        bail!(
            "distribution.install-dir = {dir:?}: must be a relative path below the LabVIEW folder, \
             with forward slashes, like \"vi.lib/Vendor/Lib\""
        );
    }
    Ok(())
}

/// Whether `name` may be a package's name: it becomes a file name in `dist/`,
/// so nothing that leaves the folder.
pub fn check_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.contains("..")
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-.".contains(c))
    {
        bail!(
            "project.name = {name:?}: a package name takes letters, digits, '_', '-' and '.', \
             like \"vendor_lib_name\""
        );
    }
    Ok(())
}

/// A relative path that stays where it starts and that Windows keeps as
/// written: forward slashes only, no drive, and every segment [`clean_segment`].
fn clean_path(path: &str) -> bool {
    !path.contains(['\\', ':']) && path.split('/').all(clean_segment)
}

/// A path segment that is not empty, `.` or `..` (which a trailing dot or space
/// would also make, as Windows drops them) and is not a Windows device: `CON`,
/// `PRN`, `AUX`, `NUL`, `COM0`-`COM9`, `LPT0`-`LPT9` (and their superscript
/// `¹`, `²`, `³` forms), `CONIN$` and `CONOUT$`,
/// whatever the case and with or without an extension.
fn clean_segment(segment: &str) -> bool {
    if segment.is_empty() || segment.ends_with(['.', ' ']) {
        return false;
    }
    let stem = segment.split('.').next().unwrap_or_default();
    let stem = stem.trim_end_matches(' ').to_ascii_uppercase();
    let numbered = |prefix: &str| {
        stem.strip_prefix(prefix).is_some_and(|n| {
            matches!(
                n.chars().collect::<Vec<_>>()[..],
                ['0'..='9' | '¹' | '²' | '³']
            )
        })
    };
    !(matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || numbered("COM")
        || numbered("LPT"))
}

/// Hashes what is written to it, and keeps it when asked to.
struct Sink {
    hash: Sha256,
    kept: Option<Vec<u8>>,
}

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.hash.update(buf);
        if let Some(kept) = &mut self.kept {
            kept.extend_from_slice(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Entry `i`, streamed through SHA-256 (lower-case hex) under `cap` decompressed
/// bytes and what is left of the archive's `budget`; its content too when
/// `keep`. Nothing is allocated from the size the entry's header claims.
fn scan<R: Read + Seek>(
    zip: &mut ZipArchive<R>,
    i: usize,
    name: &str,
    cap: u64,
    keep: bool,
    budget: &mut u64,
) -> Result<(String, Vec<u8>)> {
    let limit = cap.min(*budget);
    let mut sink = Sink {
        hash: Sha256::new(),
        kept: keep.then(Vec::new),
    };
    let mut entry = zip
        .by_index(i)
        .with_context(|| format!("opening entry {name:?}"))?;
    let read = std::io::copy(&mut (&mut entry).take(limit + 1), &mut sink)
        .with_context(|| format!("reading entry {name:?}"))?;
    if read > limit && limit == cap {
        bail!("entry {name:?} is larger than {cap} bytes");
    }
    if read > limit {
        bail!("entry {name:?} takes the archive past the total size a package may unpack to");
    }
    *budget -= read;
    Ok((
        crate::cache::finish_hex(sink.hash),
        sink.kept.unwrap_or_default(),
    ))
}

/// `RECORD`'s lines: hash, path.
fn parse_record(bytes: &[u8]) -> Result<Vec<(String, String)>> {
    let text = std::str::from_utf8(bytes).context("RECORD is not UTF-8")?;
    let mut seen = HashSet::new();
    let mut lines = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let parsed = line.split_once("  ").filter(|(hash, path)| {
            hash.len() == 64
                && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
                && !path.is_empty()
        });
        let Some((hash, path)) = parsed else {
            bail!(
                "RECORD line {}: {line:?} is not \"<sha256>  <path>\" (the hash in lower-case hex)",
                n + 1
            );
        };
        if !seen.insert(path) {
            bail!("RECORD lists {path:?} twice");
        }
        lines.push((hash.to_string(), path.to_string()));
    }
    Ok(lines)
}

/// The archive as a [`Spec`]: one `<application>` group of the files below
/// `install-dir`, no hooks. Nothing is returned unless every entry but `RECORD`
/// is listed in it with its hash, every line of it names an entry, and every
/// entry sits where a package may put it.
pub fn read<R: Read + Seek>(zip: &mut ZipArchive<R>) -> Result<Spec> {
    read_within(zip, MAX_TOTAL_BYTES)
}

/// [`read`], with the archive allowed to unpack to `total` bytes at most.
fn read_within<R: Read + Seek>(zip: &mut ZipArchive<R>, total: u64) -> Result<Spec> {
    let names: Vec<String> = (0..zip.len())
        .map(|i| zip.name_for_index(i).unwrap_or_default().to_string())
        .collect();
    // The zip crate keeps one entry per name, so what is hashed below is what
    // `install` extracts: a second `RECORD` or `package.toml` cannot hide.
    let mut budget = total;
    let Some(record_at) = names.iter().position(|n| n == RECORD) else {
        bail!("no RECORD; this is not a valid .lvpkg");
    };
    let (_, record) = scan(zip, record_at, RECORD, MAX_META_BYTES, true, &mut budget)?;
    let record = parse_record(&record)?;

    let listed: HashMap<&str, &str> = record
        .iter()
        .map(|(hash, path)| (path.as_str(), hash.as_str()))
        .collect();
    if let Some(name) = names
        .iter()
        .find(|n| *n != RECORD && !listed.contains_key(n.as_str()))
    {
        bail!("entry {name:?} is not listed in RECORD {ALTERED}");
    }
    let present: HashSet<&str> = names.iter().map(String::as_str).collect();
    if let Some((_, path)) = record.iter().find(|(_, p)| !present.contains(p.as_str())) {
        bail!("RECORD lists {path:?}, which the archive lacks {ALTERED}");
    }

    let mut package_toml = None;
    for (i, name) in names.iter().enumerate().filter(|(_, n)| *n != RECORD) {
        let is_meta = name == PACKAGE_TOML;
        let cap = if is_meta {
            MAX_META_BYTES
        } else {
            MAX_ENTRY_BYTES
        };
        let (hex, content) = scan(zip, i, name, cap, is_meta, &mut budget)?;
        if hex != listed[name.as_str()] {
            bail!("entry {name:?} differs from RECORD {ALTERED}");
        }
        if is_meta {
            package_toml = Some(content);
        }
    }
    let Some(package_toml) = package_toml else {
        bail!("no package.toml; this is not a valid .lvpkg");
    };
    spec_from(&package_toml, &names)
}

/// `package.toml`'s `text` and the archive's entry `names` as a [`Spec`].
fn spec_from(package_toml: &[u8], names: &[String]) -> Result<Spec> {
    let text = std::str::from_utf8(package_toml).context("package.toml is not UTF-8")?;
    let toml: toml::Table = match text.parse() {
        Ok(t) => t,
        Err(e) => bail!("package.toml is not valid TOML: {e}"),
    };
    match toml.get("format") {
        Some(toml::Value::Integer(1)) => {}
        Some(toml::Value::Integer(n)) if *n > 1 => {
            bail!("package.toml format = {n} was written by a newer lvpm; update lvpm")
        }
        _ => bail!("package.toml format must be 1; this is not a valid .lvpkg"),
    }

    let project = table(&toml, "project")?;
    let name = text_of(project, "project", "name")?;
    check_name(name)?;
    let version = text_of(project, "project", "version")?;
    let labview = text_of(project, "project", "labview")?;
    let year: u32 = labview_minimum(labview)?.parse()?;

    let dist = table(&toml, "distribution")?;
    let kind = text_of(dist, "distribution", "kind")?;
    if kind != "source" {
        bail!("package.toml distribution.kind = {kind:?}: this version reads only \"source\"");
    }
    text_of(dist, "distribution", "vendor")?;
    let display_name = text_of(dist, "distribution", "display-name")?;
    let install_dir = text_of(dist, "distribution", "install-dir")?;
    check_install_dir(install_dir)?;

    let mut requires = Vec::new();
    if let Some(deps) = toml.get("dependencies") {
        let Some(deps) = deps.as_table() else {
            bail!("package.toml dependencies must be a table");
        };
        for (dep, value) in deps {
            let Some(constraint) = value.as_str() else {
                bail!("package.toml dependencies.{dep} must be a string");
            };
            let item = requires_item(
                dep,
                &Constraint::parse(constraint)
                    .with_context(|| format!("package.toml dependencies.{dep}"))?,
            );
            if item.contains(',') {
                bail!(
                    "package.toml dependencies.{dep}: {item:?} holds a ',', which would split the Requires list"
                );
            }
            requires.push(item);
        }
    }

    let prefix = format!("{install_dir}/");
    let mut lower: HashMap<String, &str> = HashMap::new();
    let mut files = Vec::new();
    for entry in names.iter().filter(|n| *n != RECORD && *n != PACKAGE_TOML) {
        if entry.ends_with(['/', '\\']) {
            bail!("entry {entry:?} is a directory; a .lvpkg holds files only");
        }
        let below = entry.strip_prefix(&prefix).filter(|rel| !rel.is_empty());
        let Some(rel) = below else {
            bail!("entry {entry:?} is not below install-dir {install_dir:?}");
        };
        if !clean_path(rel) {
            bail!("entry {entry:?} cannot be installed: {BAD_PATH}");
        }
        if let Some(first) = lower.insert(entry.to_lowercase(), entry) {
            bail!("entries {first:?} and {entry:?} differ only in letter case");
        }
        files.push(entry.clone());
    }

    Ok(Spec {
        name: name.to_string(),
        version: version.to_string(),
        display_name: Some(display_name.to_string()),
        file_groups: vec![FileGroup {
            index: 0,
            target_dir: "<application>".into(),
            replace_mode: "Always".into(),
            files,
            entry_prefix: String::new(),
        }],
        script_vis: Vec::new(),
        requires: (!requires.is_empty()).then(|| requires.join(",")),
        lv_gate: Some(lv_gate(year)),
        description: Default::default(),
    })
}

fn table<'a>(t: &'a toml::Table, key: &str) -> Result<&'a toml::Table> {
    match t.get(key).and_then(toml::Value::as_table) {
        Some(table) => Ok(table),
        None => bail!("package.toml has no [{key}] table"),
    }
}

fn text_of<'a>(t: &'a toml::Table, table: &str, key: &str) -> Result<&'a str> {
    match t.get(key).and_then(toml::Value::as_str) {
        Some(s) if !s.trim().is_empty() => Ok(s),
        _ => bail!("package.toml {table}.{key} must be a non-empty string"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> Meta {
        Meta {
            name: "demo_lib".into(),
            version: "1.2.3".into(),
            labview: ">=2016".into(),
            vendor: "V".into(),
            display_name: "D".into(),
            install_dir: "vi.lib/V/D".into(),
            dependencies: Vec::new(),
        }
    }

    fn files(names: &[&str]) -> Vec<(String, Vec<u8>)> {
        names
            .iter()
            .map(|n| (n.to_string(), format!("content of {n}").into_bytes()))
            .collect()
    }

    /// Every entry of `bytes` in archive order: name, time, content.
    fn read_all(bytes: &[u8]) -> Vec<(String, zip::DateTime, Vec<u8>)> {
        use std::io::Read;
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        (0..zip.len())
            .map(|i| {
                let mut f = zip.by_index(i).unwrap();
                let mut content = Vec::new();
                f.read_to_end(&mut content).unwrap();
                (f.name().to_string(), f.last_modified().unwrap(), content)
            })
            .collect()
    }

    fn names(bytes: &[u8]) -> Vec<String> {
        read_all(bytes).into_iter().map(|(n, ..)| n).collect()
    }

    #[test]
    fn entries_are_sorted_with_record_and_package_toml_first() {
        let bytes = write(&meta(), &files(&["b.vi", "a.vi"])).unwrap();
        assert_eq!(
            names(&bytes),
            [
                "RECORD",
                "package.toml",
                "vi.lib/V/D/a.vi",
                "vi.lib/V/D/b.vi"
            ]
        );
    }

    #[test]
    fn payload_is_sorted_byte_wise_not_by_locale() {
        let bytes = write(
            &meta(),
            &files(&["d.vi", "B.vi", "a.vi", "sub/c.vi", "_x.vi"]),
        )
        .unwrap();
        assert_eq!(
            names(&bytes)[2..],
            [
                "vi.lib/V/D/B.vi",
                "vi.lib/V/D/_x.vi",
                "vi.lib/V/D/a.vi",
                "vi.lib/V/D/d.vi",
                "vi.lib/V/D/sub/c.vi"
            ]
        );
    }

    #[test]
    fn writing_is_deterministic_and_every_entry_is_dated_1980() {
        let a = write(&meta(), &files(&["b.vi", "a.vi"])).unwrap();
        let b = write(&meta(), &files(&["a.vi", "b.vi"])).unwrap();
        assert_eq!(a, b, "the order the files are given in does not matter");
        for (name, time, _) in read_all(&a) {
            assert_eq!(
                (
                    time.year(),
                    time.month(),
                    time.day(),
                    time.hour(),
                    time.minute(),
                    time.second()
                ),
                (1980, 1, 1, 0, 0, 0),
                "{name}"
            );
        }
    }

    #[test]
    fn package_toml_is_written_in_a_fixed_order_with_dependencies_as_given() {
        let mut m = meta();
        m.dependencies = vec![
            ("a".into(), "1.0".into()),
            ("b".into(), "*".into()),
            ("@o/c".into(), ">=2.0".into()),
        ];
        assert_eq!(
            m.to_toml(),
            "format = 1\n\
             \n\
             [project]\n\
             name = \"demo_lib\"\n\
             version = \"1.2.3\"\n\
             labview = \">=2016\"\n\
             \n\
             [distribution]\n\
             kind = \"source\"\n\
             vendor = \"V\"\n\
             display-name = \"D\"\n\
             install-dir = \"vi.lib/V/D\"\n\
             \n\
             [dependencies]\n\
             a = \"1.0\"\n\
             b = \"*\"\n\
             \"@o/c\" = \">=2.0\"\n"
        );
    }

    #[test]
    fn record_lists_every_entry_but_itself_in_entry_order_with_lower_case_sha256() {
        let mut f = files(&["b.vi"]);
        f.push(("a.vi".into(), b"abc".to_vec()));
        let bytes = write(&meta(), &f).unwrap();
        let all = read_all(&bytes);
        let record = String::from_utf8(all[0].2.clone()).unwrap();
        let lines: Vec<&str> = record.lines().collect();
        assert_eq!(lines.len(), all.len() - 1);
        // SHA-256("abc"), from FIPS 180-2, not computed by the code under test.
        assert_eq!(
            lines[1],
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  vi.lib/V/D/a.vi"
        );
        for (line, (name, _, content)) in lines.iter().zip(&all[1..]) {
            let (hash, path) = line.split_once("  ").unwrap();
            assert_eq!(path, name);
            assert_eq!(hash, crate::cache::sha256_hex(content), "{name}");
            assert!(hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));
        }
        assert!(record.ends_with('\n'));
    }

    // --- reading ---

    type Entries = Vec<(String, Vec<u8>)>;

    /// A written archive's entries, in archive order.
    fn written() -> Entries {
        let bytes = write(&meta(), &files(&["a.vi", "b.vi"])).unwrap();
        read_all(&bytes)
            .into_iter()
            .map(|(name, _, content)| (name, content))
            .collect()
    }

    /// `entries` as a zip, verbatim: whatever `RECORD` they hold stays.
    fn zip_of(entries: &Entries) -> Vec<u8> {
        let options = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .last_modified_time(zip::DateTime::default());
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, content) in entries {
            if name.ends_with('/') {
                zip.add_directory(name.as_str(), options).unwrap();
            } else {
                zip.start_file(name.as_str(), options).unwrap();
                zip.write_all(content).unwrap();
            }
        }
        zip.finish().unwrap().into_inner()
    }

    /// `entries` with a `RECORD` that matches them, as the first entry.
    fn with_record(mut entries: Entries) -> Entries {
        entries.retain(|(name, _)| name != RECORD);
        let record: String = entries
            .iter()
            .map(|(name, content)| format!("{}  {name}\n", crate::cache::sha256_hex(content)))
            .collect();
        entries.insert(0, (RECORD.into(), record.into_bytes()));
        entries
    }

    fn set(entries: &mut Entries, name: &str, content: &[u8]) {
        let slot = entries.iter_mut().find(|(n, _)| n == name).unwrap();
        slot.1 = content.to_vec();
    }

    fn read_bytes(bytes: Vec<u8>) -> Result<Spec> {
        read(&mut ZipArchive::new(Cursor::new(bytes)).unwrap())
    }

    fn refusal(entries: &Entries) -> String {
        format!("{:#}", read_bytes(zip_of(entries)).unwrap_err())
    }

    /// The tail every message about a package that disagrees with its RECORD ends in.
    fn altered(what: &str) -> String {
        format!("{what} (the package was altered or is corrupt); rebuild or download it again")
    }

    fn toml_of(entries: &Entries) -> String {
        let (_, content) = entries.iter().find(|(n, _)| n == PACKAGE_TOML).unwrap();
        String::from_utf8(content.clone()).unwrap()
    }

    #[test]
    fn package_toml_reads_back() {
        let mut m = meta();
        m.dependencies = vec![
            ("a".into(), "1.0".into()),
            ("b".into(), "*".into()),
            ("@o/c".into(), ">=2.0".into()),
        ];
        let s = read_bytes(write(&m, &files(&["b.vi", "a.vi"])).unwrap()).unwrap();
        assert_eq!((s.name.as_str(), s.version.as_str()), ("demo_lib", "1.2.3"));
        assert_eq!(s.lv_gate.as_deref(), Some("LabVIEW>=16.0"));
        assert_eq!(s.display_name.as_deref(), Some("D"));
        assert_eq!(s.requires.as_deref(), Some("a>=1.0,b,@o/c>=2.0"));
        assert!(s.script_vis.is_empty());
        let [group] = &s.file_groups[..] else {
            panic!("one group, got {}", s.file_groups.len());
        };
        assert_eq!(group.target_dir, "<application>");
        assert_eq!(group.replace_mode, "Always");
        assert_eq!(group.files, ["vi.lib/V/D/a.vi", "vi.lib/V/D/b.vi"]);
        assert_eq!(group.entry("vi.lib/V/D/a.vi"), "vi.lib/V/D/a.vi");
    }

    #[test]
    fn a_package_without_dependencies_has_no_requires() {
        let s = read_bytes(write(&meta(), &files(&["a.vi"])).unwrap()).unwrap();
        assert_eq!(s.requires, None);
    }

    #[test]
    fn the_install_plan_finds_every_payload_file() {
        let dir = std::env::temp_dir().join(format!("lvpm-lvpkg-plan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let roots = crate::target::Roots::scratch(&dir);
        let mut archive =
            crate::install::open_archive(write(&meta(), &files(&["a.vi", "b.vi"])).unwrap())
                .unwrap();
        let spec = read(&mut archive).unwrap();
        let plan = crate::install::plan(&roots, &spec, &mut archive).unwrap();
        assert!(plan.missing_from_archive.is_empty(), "{plan:?}");
        assert_eq!(plan.writes.len(), 2);
        crate::install::apply(&roots, &spec, &mut archive, &plan, None, "sha").unwrap();
        let installed = dir.join("LabVIEW/vi.lib/V/D");
        assert_eq!(
            std::fs::read_to_string(installed.join("a.vi")).unwrap(),
            "content of a.vi"
        );
        assert_eq!(
            std::fs::read_to_string(installed.join("b.vi")).unwrap(),
            "content of b.vi"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_archive_that_disagrees_with_its_record_is_refused() {
        type Change = fn(&mut Entries);
        let cases: [(&str, Change, String); 5] = [
            (
                "a.vi has other bytes",
                |e| set(e, "vi.lib/V/D/a.vi", b"other"),
                altered("entry \"vi.lib/V/D/a.vi\" differs from RECORD"),
            ),
            (
                "package.toml has other bytes",
                |e| {
                    let text = toml_of(e) + "# other\n";
                    set(e, PACKAGE_TOML, text.as_bytes());
                },
                altered("entry \"package.toml\" differs from RECORD"),
            ),
            (
                "an entry is added",
                |e| e.push(("vi.lib/V/D/x.vi".into(), b"x".to_vec())),
                altered("entry \"vi.lib/V/D/x.vi\" is not listed in RECORD"),
            ),
            (
                "an entry is removed",
                |e| e.retain(|(n, _)| n != "vi.lib/V/D/b.vi"),
                altered("RECORD lists \"vi.lib/V/D/b.vi\", which the archive lacks"),
            ),
            (
                "RECORD is removed",
                |e| e.retain(|(n, _)| n != RECORD),
                "no RECORD; this is not a valid .lvpkg".into(),
            ),
        ];
        for (change, edit, message) in cases {
            let mut entries = written();
            edit(&mut entries);
            assert_eq!(refusal(&entries), message, "{change}");
        }
    }

    #[test]
    fn a_structurally_bad_archive_is_refused_naming_what_is_wrong() {
        type Change = fn(&mut Entries);
        let cases: [(&str, Change, &str); 16] = [
            (
                "install-dir = ../x",
                |e| {
                    let text = toml_of(e).replace("vi.lib/V/D\"", "../x\"");
                    set(e, PACKAGE_TOML, text.as_bytes());
                },
                "distribution.install-dir = \"../x\": must be a relative path",
            ),
            (
                "install-dir = C:/x",
                |e| {
                    let text = toml_of(e).replace("vi.lib/V/D\"", "C:/x\"");
                    set(e, PACKAGE_TOML, text.as_bytes());
                },
                "distribution.install-dir = \"C:/x\": must be a relative path",
            ),
            (
                "name = a/b",
                |e| {
                    let text = toml_of(e).replace("demo_lib", "a/b");
                    set(e, PACKAGE_TOML, text.as_bytes());
                },
                "project.name = \"a/b\": a package name takes letters",
            ),
            (
                "no version",
                |e| {
                    let text = toml_of(e).replace("version = \"1.2.3\"\n", "");
                    set(e, PACKAGE_TOML, text.as_bytes());
                },
                "package.toml project.version must be a non-empty string",
            ),
            (
                "format = 2",
                |e| {
                    let text = toml_of(e).replace("format = 1", "format = 2");
                    set(e, PACKAGE_TOML, text.as_bytes());
                },
                "package.toml format = 2 was written by a newer lvpm",
            ),
            (
                "not TOML",
                |e| set(e, PACKAGE_TOML, b"this is = = not toml"),
                "package.toml is not valid TOML",
            ),
            (
                "an entry outside install-dir",
                |e| e.push(("other/x.vi".into(), b"x".to_vec())),
                "entry \"other/x.vi\" is not below install-dir \"vi.lib/V/D\"",
            ),
            (
                "a path with ..",
                |e| e.push(("vi.lib/V/D/../x.vi".into(), b"x".to_vec())),
                "entry \"vi.lib/V/D/../x.vi\" cannot be installed",
            ),
            (
                "a Windows device name",
                |e| e.push(("vi.lib/V/D/con.txt".into(), b"x".to_vec())),
                "entry \"vi.lib/V/D/con.txt\" cannot be installed",
            ),
            (
                "a name that ends in a dot",
                |e| e.push(("vi.lib/V/D/x.".into(), b"x".to_vec())),
                "entry \"vi.lib/V/D/x.\" cannot be installed",
            ),
            (
                "a directory",
                |e| e.push(("vi.lib/V/D/sub/".into(), Vec::new())),
                "entry \"vi.lib/V/D/sub/\" is a directory",
            ),
            (
                "names that differ in letter case",
                |e| e.push(("vi.lib/V/D/A.vi".into(), b"x".to_vec())),
                "entries \"vi.lib/V/D/a.vi\" and \"vi.lib/V/D/A.vi\" differ only in letter case",
            ),
            (
                "a kind that is not source",
                |e| {
                    let text = toml_of(e).replace("kind = \"source\"", "kind = \"ppl\"");
                    set(e, PACKAGE_TOML, text.as_bytes());
                },
                "package.toml distribution.kind = \"ppl\": this version reads only \"source\"",
            ),
            (
                "no kind",
                |e| {
                    let text = toml_of(e).replace("kind = \"source\"\n", "");
                    set(e, PACKAGE_TOML, text.as_bytes());
                },
                "package.toml distribution.kind must be a non-empty string",
            ),
            (
                "labview without a >=",
                |e| {
                    let text = toml_of(e).replace(">=2016", "2016");
                    set(e, PACKAGE_TOML, text.as_bytes());
                },
                "project.labview = \"2016\" is a minimum",
            ),
            (
                "labview that is no year",
                |e| {
                    let text = toml_of(e).replace(">=2016", ">=soon");
                    set(e, PACKAGE_TOML, text.as_bytes());
                },
                "project.labview = \">=soon\" is not a minimum LabVIEW year",
            ),
        ];
        for (change, edit, what) in cases {
            let mut entries = written();
            edit(&mut entries);
            let why = refusal(&with_record(entries));
            assert!(why.contains(what), "{change}: {why}");
        }
    }

    #[test]
    fn a_newer_format_says_to_update_lvpm() {
        let mut entries = written();
        let text = toml_of(&entries).replace("format = 1", "format = 2");
        set(&mut entries, PACKAGE_TOML, text.as_bytes());
        assert_eq!(
            refusal(&with_record(entries)),
            "package.toml format = 2 was written by a newer lvpm; update lvpm"
        );
    }

    #[test]
    fn a_format_that_is_missing_below_one_or_not_an_integer_is_not_a_package() {
        for format in ["format = 0\n", "format = -1\n", "format = \"1\"\n", ""] {
            let mut entries = written();
            let text = toml_of(&entries).replace("format = 1\n", format);
            set(&mut entries, PACKAGE_TOML, text.as_bytes());
            assert_eq!(
                refusal(&with_record(entries)),
                "package.toml format must be 1; this is not a valid .lvpkg",
                "{format:?}"
            );
        }
    }

    #[test]
    fn a_bad_record_is_refused_naming_record() {
        let record_with = |text: &str| {
            let mut entries = written();
            set(&mut entries, RECORD, text.as_bytes());
            refusal(&entries)
        };
        assert!(record_with("vi.lib/V/D/a.vi\n").contains("RECORD line 1"));
        let entries = written();
        let record = String::from_utf8(entries[0].1.clone()).unwrap();
        let a_line = record.lines().find(|l| l.ends_with("a.vi")).unwrap();
        let why = record_with(&format!("{record}{a_line}\n"));
        assert_eq!(why, "RECORD lists \"vi.lib/V/D/a.vi\" twice");
        // a hash with a non-hex digit or of the wrong length is not a hash
        let bad_digit = format!("G{}", &record[1..]);
        assert!(record_with(&bad_digit).contains("RECORD line 1"));
        let short = record.replacen(&record[..64], &record[..63], 1);
        assert!(record_with(&short).contains("RECORD line 1"));
    }

    #[test]
    fn package_toml_is_read_up_to_one_mib_and_no_further() {
        let padded = |len: usize| {
            let mut entries = written();
            let mut text = toml_of(&entries);
            text.push_str(&"#".repeat(len - text.len() - 1));
            text.push('\n');
            set(&mut entries, PACKAGE_TOML, text.as_bytes());
            with_record(entries)
        };
        read_bytes(zip_of(&padded(1024 * 1024))).unwrap();
        let why = refusal(&padded(1024 * 1024 + 1));
        assert_eq!(why, "entry \"package.toml\" is larger than 1048576 bytes");
    }

    #[test]
    fn an_archive_without_package_toml_is_refused() {
        let mut entries = written();
        entries.retain(|(n, _)| n != PACKAGE_TOML);
        assert_eq!(
            refusal(&with_record(entries)),
            "no package.toml; this is not a valid .lvpkg"
        );
    }

    #[test]
    fn a_bad_dependency_is_refused_naming_it() {
        let mut entries = written();
        let text = toml_of(&entries) + "x = \"~1.0\"\n";
        set(&mut entries, PACKAGE_TOML, text.as_bytes());
        let why = refusal(&with_record(entries));
        assert!(why.contains("dependencies.x"), "{why}");
    }

    #[test]
    fn a_refusal_for_an_altered_entry_reads_in_full() {
        let mut entries = written();
        set(&mut entries, "vi.lib/V/D/a.vi", b"other");
        assert_eq!(
            refusal(&entries),
            "entry \"vi.lib/V/D/a.vi\" differs from RECORD (the package was altered or is corrupt); \
             rebuild or download it again"
        );
    }

    #[test]
    fn check_name_and_check_install_dir_take_what_a_manifest_may_hold() {
        for ok in ["demo_lib", "a-b.c", "A1"] {
            check_name(ok).unwrap();
        }
        for bad in ["", "a/b", "..", "a..b", "@scope/lib", "a b"] {
            assert!(check_name(bad).is_err(), "{bad:?}");
        }
        for ok in ["vi.lib/V/D", "vi.lib/Vendor Name/Lib 2", "x"] {
            check_install_dir(ok).unwrap();
        }
        for bad in [
            "", "/", "/x", "../x", "a/../x", "C:/x", r"a\b", ".", "a/./b", "a//b", "a/", "a/.. /x",
        ] {
            let why = check_install_dir(bad).unwrap_err().to_string();
            let start = format!("distribution.install-dir = {bad:?}: must be a relative path");
            assert!(why.starts_with(&start), "{bad:?}: {why}");
        }
    }

    #[test]
    fn a_path_may_not_end_in_a_dot_or_space_or_name_a_windows_device() {
        for ok in [
            "a.vi",
            "console.vi",
            "COM10",
            "COM",
            "LPT",
            "com1x",
            "NULL",
            "a.b.c",
            "CON-1",
            "Acme Inc",
        ] {
            assert!(clean_segment(ok), "{ok:?} is fine");
        }
        for bad in [
            "",
            ".",
            "..",
            "a.",
            "a ",
            "a. ",
            "CON",
            "con",
            "Prn",
            "AUX.txt",
            "nul.tar.gz",
            "COM1",
            "com9.vi",
            "LPT1",
            "lpt9.x",
            "COM0",
            "lpt0.vi",
            "COM¹.vi",
            "lpt³",
            "CONIN$",
            "conout$",
            "CON .txt",
        ] {
            assert!(!clean_segment(bad), "{bad:?} is refused");
        }
        for bad in ["vi.lib/CON/x", "vi.lib/V/Aux.vi.", "x/COM3"] {
            assert!(!clean_path(bad), "{bad:?}");
        }
        assert!(check_install_dir("vi.lib/CON/x").is_err());
        assert!(check_install_dir("vi.lib/Acme Inc./x").is_err());
    }

    #[test]
    fn write_refuses_what_read_would_and_never_panics_on_it() {
        let write_with = |names: &[&str]| write(&meta(), &files(names));
        for (names, why) in [
            (
                &["../x.vi"][..],
                "member \"../x.vi\" cannot go into a package",
            ),
            (
                &["a\\b.vi"],
                "member \"a\\\\b.vi\" cannot go into a package",
            ),
            (
                &["sub/NUL.txt"],
                "member \"sub/NUL.txt\" cannot go into a package",
            ),
            (&["x."], "member \"x.\" cannot go into a package"),
            (&["a.vi", "a.vi"], "member \"a.vi\" is listed twice"),
            (
                &["a.vi", "A.vi"],
                "members \"a.vi\" and \"A.vi\" differ only in letter case",
            ),
            (
                &["A.vi", "a.vi"],
                "members \"A.vi\" and \"a.vi\" differ only in letter case",
            ),
        ] {
            let err = write_with(names).unwrap_err().to_string();
            assert!(err.starts_with(why), "{names:?}: {err}");
        }
        let mut m = meta();
        m.install_dir = "../x".into();
        let err = write(&m, &files(&["a.vi"])).unwrap_err().to_string();
        assert!(err.starts_with("distribution.install-dir"), "{err}");
    }

    #[test]
    fn letter_case_duplicates_name_both_entries_whatever_the_order() {
        for order in [
            ["vi.lib/V/D/a.vi", "vi.lib/V/D/A.vi"],
            ["vi.lib/V/D/A.vi", "vi.lib/V/D/a.vi"],
        ] {
            let mut entries = written();
            entries.retain(|(n, _)| n.ends_with("package.toml"));
            for name in order {
                entries.push((name.into(), b"x".to_vec()));
            }
            assert_eq!(
                refusal(&with_record(entries)),
                format!(
                    "entries {:?} and {:?} differ only in letter case",
                    order[0], order[1]
                )
            );
        }
    }

    #[test]
    fn two_entries_of_one_name_are_one_entry_to_the_zip_crate_so_read_checks_what_install_extracts()
    {
        // The zip writer refuses two entries of one name, so make them: a twin
        // is written under a name one character off, then renamed in both of
        // its headers.
        for name in [PACKAGE_TOML, RECORD, "vi.lib/V/D/a.vi"] {
            let mut entries = written();
            let original = entries.iter().find(|(n, _)| n == name).unwrap().1.clone();
            let mut twin = name.to_string();
            twin.pop();
            twin.push('~');
            entries.push((twin.clone(), b"other bytes".to_vec()));
            let mut bytes = zip_of(&entries);
            let (from, to) = (twin.as_bytes(), name.as_bytes());
            let mut found = 0;
            for i in 0..bytes.len() - from.len() {
                if bytes[i..i + from.len()] == *from {
                    bytes[i..i + from.len()].copy_from_slice(to);
                    found += 1;
                }
            }
            assert_eq!(found, 2, "a local header and a central one");
            let mut zip = ZipArchive::new(Cursor::new(bytes.clone())).unwrap();
            assert_eq!(zip.len(), written().len(), "{name}: one entry per name");
            let mut extracted = Vec::new();
            zip.by_name(name)
                .unwrap()
                .read_to_end(&mut extracted)
                .unwrap();
            // read hashes the entry that by_name, and so install, extracts
            assert_eq!(read_bytes(bytes).is_ok(), extracted == original, "{name}");
        }
    }

    #[test]
    fn an_archive_may_unpack_to_its_total_cap_and_no_more() {
        let entries = written();
        let total: u64 = entries.iter().map(|(_, c)| c.len() as u64).sum();
        let zipped = zip_of(&entries);
        let read_with = |cap| {
            read_within(
                &mut ZipArchive::new(Cursor::new(zipped.clone())).unwrap(),
                cap,
            )
        };
        read_with(total).unwrap();
        let why = read_with(total - 1).unwrap_err().to_string();
        assert!(
            why.contains("past the total size a package may unpack to"),
            "{why}"
        );
    }

    #[test]
    fn read_spec_reads_by_content_a_package_toml_as_a_lvpkg() {
        let mut archive =
            crate::install::open_archive(write(&meta(), &files(&["a.vi"])).unwrap()).unwrap();
        let s = crate::install::read_spec(&mut archive).unwrap();
        assert_eq!(s.name, "demo_lib");
        // and checks its RECORD on the way
        let mut entries = written();
        set(&mut entries, "vi.lib/V/D/a.vi", b"other");
        let mut archive = crate::install::open_archive(zip_of(&entries)).unwrap();
        let why = crate::install::read_spec(&mut archive)
            .unwrap_err()
            .to_string();
        assert!(why.contains("differs from RECORD"), "{why}");
    }

    #[test]
    fn read_spec_refuses_an_archive_with_both_package_toml_and_spec() {
        let mut entries = written();
        entries.push((
            "spec".into(),
            b"[Package]\nName=alpha\nVersion=1.0\n".to_vec(),
        ));
        let mut archive = crate::install::open_archive(zip_of(&with_record(entries))).unwrap();
        let why = crate::install::read_spec(&mut archive)
            .unwrap_err()
            .to_string();
        assert!(why.contains("both package.toml and spec"), "{why}");
    }
}
