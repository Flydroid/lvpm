//! The `.vip` variants the probe installs, and the ID one of them carries.

use crate::support::snapshot::hex;
use crate::vipm::DATA_MANIPULATION;
use md5::{Digest, Md5};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use zip::write::SimpleFileOptions;

/// A package ID derived from what the package holds: MD5 over
/// `name\0version\0`, then `path\0md5(content)\n` for every entry but `spec`,
/// sorted by path (`/` separators). `spec` is left out because it will hold
/// the ID itself, which would make the ID circular. Two builds of the same
/// files get the same ID, so builds stay byte-identical.
pub fn package_id(name: &str, version: &str, entries: &[(String, Vec<u8>)]) -> String {
    let mut files: Vec<(String, &[u8])> = entries
        .iter()
        .filter(|(path, _)| path != "spec")
        .map(|(path, bytes)| (path.replace('\\', "/"), bytes.as_slice()))
        .collect();
    files.sort();
    let mut hash = Md5::new();
    hash.update(format!("{name}\0{version}\0"));
    for (path, bytes) in files {
        hash.update(format!("{path}\0{}\n", hex(&Md5::digest(bytes))));
    }
    hex(&hash.finalize())
}

/// A package as the probe edits it: its zip entries in order, `spec` first.
/// A directory entry is a name ending in `/` with no bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct Pkg {
    pub entries: Vec<(String, Vec<u8>)>,
}

impl Pkg {
    pub fn read(zip_bytes: &[u8]) -> Result<Pkg, String> {
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes))
            .map_err(|e| format!("not a zip: {e}"))?;
        let mut entries = Vec::new();
        for i in 0..zip.len() {
            let mut file = zip.by_index(i).map_err(|e| format!("entry {i}: {e}"))?;
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)
                .map_err(|e| format!("reading {}: {e}", file.name()))?;
            entries.push((file.name().to_string(), bytes));
        }
        Ok(Pkg { entries })
    }

    /// The zip, written the way `lvpm build` writes one: fixed timestamps and one Deflate level.
    pub fn to_zip(&self) -> Result<Vec<u8>, String> {
        let options = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .compression_level(Some(6))
            .last_modified_time(zip::DateTime::default());
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, bytes) in &self.entries {
            let written = match name.ends_with('/') {
                true => zip.add_directory(name.as_str(), options),
                false => zip
                    .start_file(name.as_str(), options)
                    .and_then(|()| zip.write_all(bytes).map_err(Into::into)),
            };
            written.map_err(|e| format!("writing {name}: {e}"))?;
        }
        let done = zip
            .finish()
            .map_err(|e| format!("finishing the zip: {e}"))?;
        Ok(done.into_inner())
    }

    /// `spec` as text, one char per byte: published specs are not UTF-8, and
    /// a copied value must keep its bytes.
    pub fn spec(&self) -> String {
        self.get("spec")
            .map(|bytes| bytes.iter().map(|&b| b as char).collect())
            .unwrap_or_default()
    }

    fn set_spec(&mut self, text: &str) {
        self.put("spec", text.chars().map(|c| c as u8).collect());
    }

    pub fn get(&self, name: &str) -> Option<&[u8]> {
        self.entries
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, bytes)| bytes.as_slice())
    }

    fn put(&mut self, name: &str, bytes: Vec<u8>) {
        match self.entries.iter_mut().find(|(n, _)| n == name) {
            Some(entry) => entry.1 = bytes,
            None => self.entries.push((name.to_string(), bytes)),
        }
    }
}

/// `(section, key)` of a spec line.
type Key = (String, String);

/// A spec's `key=value` lines in order, the value as written (quotes
/// included). The numbered `File <n>` lines of a file group are left out: the
/// entry list says more about them.
pub fn spec_keys(text: &str) -> Vec<(Key, String)> {
    let mut keys: Vec<(Key, String)> = Vec::new();
    let mut section = String::new();
    for line in text.lines().map(str::trim) {
        if line.starts_with(';') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name.to_string();
        } else if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let numbered_file = section.starts_with("File Group ")
                && key
                    .strip_prefix("File ")
                    .is_some_and(|n| n.parse::<usize>().is_ok());
            if !numbered_file {
                keys.push(((section.clone(), key.to_string()), value.trim().to_string()));
            }
        }
    }
    keys
}

fn is_header(line: &str) -> bool {
    let line = line.trim();
    line.starts_with('[') && line.ends_with(']')
}

/// `text` with `key` of `section` set to `value` as written, or removed when
/// `value` is `None`. Other lines stay as they are; a missing section is added at the end.
fn set_key(text: &str, section: &str, key: &str, value: Option<&str>) -> String {
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    let header = format!("[{section}]");
    let new_line = value.map(|v| format!("{key}={v}"));
    match lines.iter().position(|l| l.trim() == header) {
        None => {
            if let Some(line) = new_line {
                lines.extend([String::new(), header, line]);
            }
        }
        Some(start) => {
            let end = (start + 1..lines.len())
                .find(|&i| is_header(&lines[i]))
                .unwrap_or(lines.len());
            let existing = (start + 1..end).find(|&i| {
                lines[i]
                    .split_once('=')
                    .is_some_and(|(k, _)| k.trim() == key)
            });
            match (existing, new_line) {
                (Some(i), Some(line)) => lines[i] = line,
                (Some(i), None) => drop(lines.remove(i)),
                (None, Some(line)) => {
                    let last = (start..end)
                        .rev()
                        .find(|&i| !lines[i].trim().is_empty())
                        .unwrap_or(start);
                    lines.insert(last + 1, line);
                }
                (None, None) => {}
            }
        }
    }
    lines.iter().map(|l| format!("{l}\n")).collect()
}

/// A change to a spec key: `None` removes it.
type SetKey = (String, String, Option<String>);

/// One `.vip` that differs from the base in what it names.
#[derive(Debug, Clone)]
pub struct Variant {
    pub name: String,
    keys: Vec<SetKey>,
    entries: Vec<(String, Vec<u8>)>,
}

impl Variant {
    pub fn new(name: &str) -> Variant {
        Variant {
            name: name.to_string(),
            keys: Vec::new(),
            entries: Vec::new(),
        }
    }

    pub fn key(mut self, section: &str, key: &str, value: Option<&str>) -> Variant {
        let value = value.map(String::from);
        self.keys
            .push((section.to_string(), key.to_string(), value));
        self
    }

    pub fn entry(mut self, name: &str, bytes: &[u8]) -> Variant {
        self.entries.push((name.to_string(), bytes.to_vec()));
        self
    }

    pub fn apply(&self, base: &Pkg) -> Pkg {
        let mut pkg = base.clone();
        let mut spec = pkg.spec();
        for (section, key, value) in &self.keys {
            spec = set_key(&spec, section, key, value.as_deref());
        }
        pkg.set_spec(&spec);
        for (name, bytes) in &self.entries {
            pkg.put(name, bytes.clone());
        }
        pkg
    }

    /// What this variant says it changes, as `spec:[Section]/Key` and `entry:<name>`.
    pub fn changes(&self) -> BTreeSet<String> {
        let keys = self.keys.iter().map(|(s, k, _)| format!("spec:[{s}]/{k}"));
        let entries = self.entries.iter().map(|(n, _)| format!("entry:{n}"));
        keys.chain(entries).collect()
    }
}

/// What differs between two packages: the spec keys (added, removed or with
/// another value) and the entries other than `spec`.
#[cfg(test)]
fn differences(a: &Pkg, b: &Pkg) -> BTreeSet<String> {
    let keys = |p: &Pkg| -> BTreeMap<Key, String> { spec_keys(&p.spec()).into_iter().collect() };
    let (ka, kb) = (keys(a), keys(b));
    let mut found = BTreeSet::new();
    for key in ka.keys().chain(kb.keys()) {
        if ka.get(key) != kb.get(key) {
            found.insert(format!("spec:[{}]/{}", key.0, key.1));
        }
    }
    let files = |p: &Pkg| -> BTreeMap<String, Vec<u8>> {
        p.entries
            .iter()
            .filter(|(n, _)| n != "spec")
            .cloned()
            .collect()
    };
    let (fa, fb) = (files(a), files(b));
    for name in fa.keys().chain(fb.keys()) {
        if fa.get(name) != fb.get(name) {
            found.insert(format!("entry:{name}"));
        }
    }
    found
}

/// A fixed-seed xorshift64, so the "random" ID is the same on every run
/// without a dependency.
fn random_id() -> String {
    let mut x: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    format!("{:016x}{:016x}", next(), next())
}

/// How many keys the published specs share and lvpm does not write that get a variant.
const MAX_KEY_VARIANTS: usize = 5;
/// Keys with a variant of their own.
const OWN_VARIANT_KEYS: [(&str, &str); 2] =
    [("Package", "ID"), ("Description", "License Agreement")];
const REQUIRES_VERSION: &str = "1.5.0.15";

/// The variants of `base` (lvpm's build of pilot A), and the keys every one
/// of `published` carries that lvpm does not write but that got no variant.
/// `reference` is pilot A's published archive; its icon, license and key
/// values are what a variant adds.
pub fn variants(
    base: &Pkg,
    reference: &Pkg,
    published: &[Pkg],
) -> Result<(Vec<Variant>, Vec<String>), String> {
    let spec = lvpm_core::spec::parse(&base.spec()).map_err(|e| format!("the base spec: {e:#}"))?;
    let mut list = id_variants(&spec.name, &spec.version, base);
    list.extend(format_variants(base, published));
    list.extend(reference_variants(reference)?);
    list.extend(requires_variants());
    let (keyed, not_varied) = published_key_variants(base, reference, published);
    list.extend(keyed);
    Ok((list, not_varied))
}

/// `base` as it is, and the three ways a package can carry an ID.
fn id_variants(name: &str, version: &str, base: &Pkg) -> Vec<Variant> {
    let id = |kind: &str, value: &str| {
        Variant::new(&format!("ID={kind}")).key("Package", "ID", Some(value))
    };
    vec![
        Variant::new("base"),
        id("content", &package_id(name, version, &base.entries)),
        id("random", &random_id()),
        id("empty", "\"\""),
    ]
}

/// `Format Version` absent, and set to each other value the published specs carry.
fn format_variants(base: &Pkg, published: &[Pkg]) -> Vec<Variant> {
    let own_format = spec_keys(&base.spec())
        .into_iter()
        .find(|(k, _)| k.1 == "Format Version")
        .map(|(_, v)| v);
    let other_formats: BTreeSet<String> = published
        .iter()
        .flat_map(|p| spec_keys(&p.spec()))
        .filter(|(k, v)| k.1 == "Format Version" && Some(v) != own_format.as_ref())
        .map(|(_, v)| v)
        .collect();
    let mut list =
        vec![Variant::new("Format Version=absent").key("Package", "Format Version", None)];
    for format in other_formats {
        let name = format!("Format Version={}", format.trim_matches('"'));
        list.push(Variant::new(&name).key("Package", "Format Version", Some(&format)));
    }
    list
}

/// The icon, and the license with the key that names it, of the published
/// archive: what lvpm's build lacks.
fn reference_variants(reference: &Pkg) -> Result<Vec<Variant>, String> {
    let icon = reference
        .get("icon.bmp")
        .ok_or("the reference has no icon.bmp")?;
    let license = reference
        .get("license")
        .ok_or("the reference has no license")?;
    let agreement = spec_keys(&reference.spec())
        .into_iter()
        .find(|(k, _)| k.1 == "License Agreement")
        .map(|(_, v)| v)
        .ok_or("the reference has no License Agreement")?;
    Ok(vec![
        Variant::new("icon.bmp=present").entry("icon.bmp", icon),
        Variant::new("license=present")
            .entry("license", license)
            .key("Description", "License Agreement", Some(&agreement)),
    ])
}

/// Data Manipulation required with each version operator.
fn requires_variants() -> Vec<Variant> {
    [">=", "="]
        .iter()
        .map(|operator| {
            let requires = format!("\"{DATA_MANIPULATION}{operator}{REQUIRES_VERSION}\"");
            Variant::new(&format!("Requires={operator}")).key(
                "Dependencies",
                "Requires",
                Some(&requires),
            )
        })
        .collect()
}

/// A variant for each of the first `MAX_KEY_VARIANTS` keys every published
/// spec carries that lvpm does not write, and the names of the rest.
fn published_key_variants(
    base: &Pkg,
    reference: &Pkg,
    published: &[Pkg],
) -> (Vec<Variant>, Vec<String>) {
    let ours: BTreeSet<Key> = spec_keys(&base.spec())
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    let in_all: Vec<BTreeSet<Key>> = published
        .iter()
        .map(|p| spec_keys(&p.spec()).into_iter().map(|(k, _)| k).collect())
        .collect();
    let reference_keys = spec_keys(&reference.spec());
    // The spec rules `[Description]` fields out of scope, so they come last.
    let mut missing: Vec<&(Key, String)> = reference_keys
        .iter()
        .filter(|(k, _)| !ours.contains(k) && in_all.iter().all(|keys| keys.contains(k)))
        .filter(|(k, _)| !OWN_VARIANT_KEYS.contains(&(k.0.as_str(), k.1.as_str())))
        .collect();
    missing.sort_by_key(|(k, _)| k.0 == "Description");
    let (mut list, mut not_varied) = (Vec::new(), Vec::new());
    for (i, ((section, key), value)) in missing.into_iter().enumerate() {
        if i < MAX_KEY_VARIANTS {
            list.push(Variant::new(&format!("[{section}]/{key}")).key(section, key, Some(value)));
        } else {
            not_varied.push(format!("[{section}]/{key}"));
        }
    }
    (list, not_varied)
}

/// Where lvpm's build and a published archive differ.
#[derive(Debug, Default, PartialEq)]
pub struct RefDiff {
    pub only_ours: Vec<String>,
    pub only_reference: Vec<String>,
    pub keys_only_ours: Vec<String>,
    pub keys_only_reference: Vec<String>,
    /// `(key, ours, reference)`
    pub keys_changed: Vec<(String, String, String)>,
}

pub fn reference_diff(ours: &Pkg, reference: &Pkg) -> RefDiff {
    let names =
        |p: &Pkg| -> BTreeSet<String> { p.entries.iter().map(|(n, _)| n.clone()).collect() };
    let keys = |p: &Pkg| -> BTreeMap<String, String> {
        spec_keys(&p.spec())
            .into_iter()
            .map(|((s, k), v)| (format!("[{s}]/{k}"), v))
            .collect()
    };
    let (na, nb) = (names(ours), names(reference));
    let (ka, kb) = (keys(ours), keys(reference));
    RefDiff {
        only_ours: na.difference(&nb).cloned().collect(),
        only_reference: nb.difference(&na).cloned().collect(),
        keys_only_ours: ka
            .keys()
            .filter(|k| !kb.contains_key(*k))
            .cloned()
            .collect(),
        keys_only_reference: kb
            .keys()
            .filter(|k| !ka.contains_key(*k))
            .cloned()
            .collect(),
        keys_changed: ka
            .iter()
            .filter_map(|(k, v)| {
                Some((k.clone(), v.clone(), kb.get(k).filter(|w| *w != v)?.clone()))
            })
            .collect(),
    }
}

#[cfg(test)]
const SAMPLE_SPEC: &str = "[Package]\nName=\"p\"\nVersion=\"1.0.0.1\"\nDisplay Name=\"P\"\n\n\
    [File Group 0]\nTarget Dir=\"<application>\"\nReplace Mode=\"Always\"\nNum Files=1\n\
    File 0=\"vi.lib/x/a.vi\"\n";

/// What `lvpm build` would write for a one-file package.
#[cfg(test)]
pub fn sample_base() -> Pkg {
    let spec = lvpm_core::spec::write(&lvpm_core::spec::parse(SAMPLE_SPEC).unwrap());
    Pkg {
        entries: vec![
            ("spec".into(), spec.into_bytes()),
            ("File Group 0/vi.lib/x/a.vi".into(), b"vi".to_vec()),
        ],
    }
}

/// Pilot A's published archive, and every published pilot archive (it too).
#[cfg(test)]
pub fn reference_and_published() -> (Pkg, Vec<Pkg>) {
    use crate::support::pilots::{PILOT_A_ARCHIVE, archives, pilots_dir};
    let read = |path: &std::path::Path| Pkg::read(&std::fs::read(path).unwrap()).unwrap();
    let all: Vec<Pkg> = archives(&pilots_dir()).iter().map(|p| read(p)).collect();
    (read(&pilots_dir().join(PILOT_A_ARCHIVE)), all)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries() -> Vec<(String, Vec<u8>)> {
        vec![
            ("spec".into(), b"[Package]".to_vec()),
            ("File Group 0/vi.lib/a.vi".into(), b"aaa".to_vec()),
            ("File Group 0/vi.lib/b.vi".into(), b"bbb".to_vec()),
        ]
    }

    #[test]
    fn the_same_input_gives_the_same_id() {
        assert_eq!(
            package_id("p", "1.0.0.1", &entries()),
            package_id("p", "1.0.0.1", &entries())
        );
    }

    /// Worked out from the algorithm in `package_id`'s doc, not by this code.
    #[test]
    fn the_id_of_known_entries_is_pinned() {
        assert_eq!(
            package_id("p", "1.0.0.1", &entries()),
            "d5ef7ca7915f6d5b72ac8bff01a116b3"
        );
    }

    #[test]
    fn backslash_and_slash_paths_give_the_same_id() {
        let mut backslashed = entries();
        backslashed[1].0 = "File Group 0\\vi.lib\\a.vi".into();
        backslashed[2].0 = "File Group 0\\vi.lib\\b.vi".into();
        assert_eq!(
            package_id("p", "1.0.0.1", &entries()),
            package_id("p", "1.0.0.1", &backslashed)
        );
    }

    #[test]
    fn the_id_is_32_lowercase_hex_digits() {
        let id = package_id("p", "1.0.0.1", &entries());
        assert_eq!(id.len(), 32);
        assert!(
            id.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
            "{id}"
        );
    }

    #[test]
    fn one_changed_member_changes_the_id() {
        let mut changed = entries();
        changed[2].1 = b"bbc".to_vec();
        assert_ne!(
            package_id("p", "1.0.0.1", &entries()),
            package_id("p", "1.0.0.1", &changed)
        );
    }

    #[test]
    fn the_order_of_the_entries_does_not_matter() {
        let mut reversed = entries();
        reversed.reverse();
        assert_eq!(
            package_id("p", "1.0.0.1", &entries()),
            package_id("p", "1.0.0.1", &reversed)
        );
    }

    #[test]
    fn a_changed_version_or_name_changes_the_id() {
        let id = package_id("p", "1.0.0.1", &entries());
        assert_ne!(id, package_id("p", "1.0.0.2", &entries()));
        assert_ne!(id, package_id("q", "1.0.0.1", &entries()));
    }

    #[test]
    fn a_changed_spec_entry_does_not_change_the_id() {
        let mut changed = entries();
        changed[0].1 = b"[Package]\nID=\"x\"".to_vec();
        assert_eq!(
            package_id("p", "1.0.0.1", &entries()),
            package_id("p", "1.0.0.1", &changed)
        );
    }

    #[test]
    fn a_renamed_member_changes_the_id() {
        let mut renamed = entries();
        renamed[1].0 = "File Group 0/vi.lib/c.vi".into();
        assert_ne!(
            package_id("p", "1.0.0.1", &entries()),
            package_id("p", "1.0.0.1", &renamed)
        );
    }

    #[test]
    fn each_variant_differs_from_the_base_in_its_one_change() {
        let (reference, all) = reference_and_published();
        let (list, _) = variants(&sample_base(), &reference, &all).unwrap();
        assert_eq!(list[0].name, "base");
        assert_eq!(
            differences(&sample_base(), &list[0].apply(&sample_base())),
            BTreeSet::new(),
            "the base differs from itself"
        );
        for variant in &list[1..] {
            let found = differences(&sample_base(), &variant.apply(&sample_base()));
            assert_eq!(found, variant.changes(), "{}", variant.name);
            // The license variant adds a file and the key that names it: one change.
            let count = if variant.name == "license=present" {
                2
            } else {
                1
            };
            assert_eq!(found.len(), count, "{}: {found:?}", variant.name);
        }
        let names: BTreeSet<&str> = list.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(names.len(), list.len(), "variant names repeat");
        for want in [
            "ID=content",
            "ID=random",
            "ID=empty",
            "Format Version=absent",
            "icon.bmp=present",
            "license=present",
            "Requires=>=",
            "Requires==",
        ] {
            assert!(names.contains(want), "no variant {want}: {names:?}");
        }
    }

    #[test]
    fn the_fixture_specs_carry_one_format_version_so_only_absent_is_varied() {
        let (reference, all) = reference_and_published();
        let (list, _) = variants(&sample_base(), &reference, &all).unwrap();
        let formats: Vec<&str> = list
            .iter()
            .map(|v| v.name.as_str())
            .filter(|n| n.starts_with("Format Version"))
            .collect();
        assert_eq!(formats, ["Format Version=absent"]);
    }

    #[test]
    fn at_most_five_published_keys_get_a_variant_and_the_rest_are_listed() {
        let (reference, all) = reference_and_published();
        let (list, not_varied) = variants(&sample_base(), &reference, &all).unwrap();
        let keyed: Vec<&str> = list
            .iter()
            .map(|v| v.name.as_str())
            .filter(|n| n.starts_with('['))
            .collect();
        assert_eq!(keyed.len(), 5, "{keyed:?}");
        assert!(keyed.contains(&"[Package]/Release"), "{keyed:?}");
        assert!(!not_varied.is_empty());
        let own = ["[Package]/ID", "[Description]/License Agreement"];
        for name in keyed.iter().map(|n| n.to_string()).chain(not_varied) {
            assert!(
                !own.contains(&name.as_str()),
                "{name} has variants of its own"
            );
        }
    }

    fn pkg_of(spec: &str) -> Pkg {
        Pkg {
            entries: vec![("spec".into(), spec.as_bytes().to_vec())],
        }
    }

    /// The names of the variants `published_key_variants` makes, and the keys it lists.
    fn keyed(base: &str, reference: &str, other: &str) -> (Vec<String>, Vec<String>) {
        let (reference, other) = (pkg_of(reference), pkg_of(other));
        let published = [reference.clone(), other];
        let (list, rest) = published_key_variants(&pkg_of(base), &reference, &published);
        (list.into_iter().map(|v| v.name).collect(), rest)
    }

    #[test]
    fn a_published_key_in_one_spec_only_or_written_by_lvpm_or_with_its_own_variant_gets_none() {
        let reference = "[Package]\nOurs=\"1\"\nID=\"abc\"\nShared=\"s\"\nOnlyHere=\"o\"\n\
                         [Description]\nLicense Agreement=\"l\"\n";
        let other = reference.replace("OnlyHere=\"o\"\n", "");
        let (names, rest) = keyed("[Package]\nOurs=\"1\"\n", reference, &other);
        assert_eq!(names, ["[Package]/Shared"]);
        assert!(rest.is_empty(), "{rest:?}");
    }

    #[test]
    fn description_keys_come_last_and_only_five_keys_get_a_variant() {
        let reference = "[Description]\nSummary=\"s\"\n[S]\nk1=\"1\"\nk2=\"2\"\nk3=\"3\"\n\
                         k4=\"4\"\nk5=\"5\"\nk6=\"6\"\n";
        let (names, rest) = keyed("[Package]\n", reference, reference);
        assert_eq!(names, ["[S]/k1", "[S]/k2", "[S]/k3", "[S]/k4", "[S]/k5"]);
        assert_eq!(rest, ["[S]/k6", "[Description]/Summary"]);
    }

    #[test]
    fn the_id_variants_set_a_32_digit_id_and_the_random_one_is_pinned() {
        let (reference, all) = reference_and_published();
        let (list, _) = variants(&sample_base(), &reference, &all).unwrap();
        let id_of = |name: &str| {
            let variant = list.iter().find(|v| v.name == name).unwrap();
            let keys = spec_keys(&variant.apply(&sample_base()).spec());
            keys.into_iter().find(|(k, _)| k.1 == "ID").unwrap().1
        };
        assert_eq!(id_of("ID=content").len(), 32);
        assert_eq!(id_of("ID=random").len(), 32);
        // Both worked out apart from this code: MD5 per `package_id`'s doc over
        // `sample_base`, and the xorshift64 steps from the seed.
        assert_eq!(id_of("ID=content"), "15c619966b02d959ee322c6068bb6684");
        assert_eq!(id_of("ID=random"), "7f6c280beaa8e3e7e47119871cf9abe0");
        assert_eq!(id_of("ID=empty"), "\"\"");
    }

    #[test]
    fn setting_a_key_replaces_adds_removes_and_creates_the_section() {
        let text = "[A]\nx=\"1\"\ny=\"2\"\n\n[B]\nz=\"3\"\n";
        assert_eq!(
            set_key(text, "A", "x", Some("\"9\"")),
            "[A]\nx=\"9\"\ny=\"2\"\n\n[B]\nz=\"3\"\n"
        );
        assert_eq!(
            set_key(text, "A", "w", Some("\"4\"")),
            "[A]\nx=\"1\"\ny=\"2\"\nw=\"4\"\n\n[B]\nz=\"3\"\n"
        );
        assert_eq!(
            set_key(text, "A", "y", None),
            "[A]\nx=\"1\"\n\n[B]\nz=\"3\"\n"
        );
        assert_eq!(set_key(text, "A", "nope", None), text);
        assert_eq!(
            set_key(text, "C", "k", Some("\"v\"")),
            "[A]\nx=\"1\"\ny=\"2\"\n\n[B]\nz=\"3\"\n\n[C]\nk=\"v\"\n"
        );
    }

    #[test]
    fn a_non_utf8_byte_in_a_spec_value_survives_a_variant_that_changes_another_key() {
        let spec = b"[Package]\nName=\"p\"\nDisplay Name=\"a\x93b\"\n".to_vec();
        let base = Pkg {
            entries: vec![("spec".into(), spec)],
        };
        let applied = Variant::new("v")
            .key("Package", "ID", Some("\"x\""))
            .apply(&base);
        let bytes = applied.get("spec").unwrap();
        assert!(bytes.windows(4).any(|w| w == b"a\x93b\""), "{bytes:?}");
        assert!(bytes.windows(3).any(|w| w == b"ID="), "{bytes:?}");
    }

    #[test]
    fn a_package_survives_a_trip_through_the_zip_with_its_bytes() {
        let pkg = Pkg {
            entries: vec![
                ("spec".into(), vec![b'a', 0x93, b'\n']),
                ("dir/".into(), Vec::new()),
                ("dir/f".into(), vec![0, 1, 2]),
            ],
        };
        let back = Pkg::read(&pkg.to_zip().unwrap()).unwrap();
        assert_eq!(back, pkg);
        assert_eq!(back.to_zip().unwrap(), pkg.to_zip().unwrap());
    }

    #[test]
    fn reference_diff_lists_one_sided_entries_and_keys() {
        let zip = |spec: &str, extra: (&str, &str)| Pkg {
            entries: vec![
                ("spec".into(), spec.as_bytes().to_vec()),
                (extra.0.into(), extra.1.as_bytes().to_vec()),
            ],
        };
        let ours = zip(
            "[Package]\nName=\"p\"\nOnlyOurs=\"1\"\nVersion=\"1\"\n",
            ("ours.txt", "o"),
        );
        let theirs = zip(
            "[Package]\nName=\"p\"\nID=abc\nVersion=\"2\"\n[File Group 0]\nFile 0=\"a\"\n",
            ("theirs.txt", "t"),
        );
        let diff = reference_diff(&ours, &theirs);
        assert_eq!(diff.only_ours, ["ours.txt"]);
        assert_eq!(diff.only_reference, ["theirs.txt"]);
        assert_eq!(diff.keys_only_ours, ["[Package]/OnlyOurs"]);
        assert_eq!(diff.keys_only_reference, ["[Package]/ID"]);
        assert_eq!(
            diff.keys_changed,
            [("[Package]/Version".into(), "\"1\"".into(), "\"2\"".into())]
        );
    }

    #[test]
    fn comment_lines_are_not_spec_keys() {
        let keys = spec_keys("; a=1\n[A]\n;b=2\n  ; c=3\nd=4\n");
        let names: Vec<(String, String)> = keys.into_iter().map(|(k, _)| k).collect();
        assert_eq!(names, [("A".to_string(), "d".to_string())]);
    }

    #[test]
    fn the_numbered_file_lines_are_not_spec_keys() {
        let keys = spec_keys(
            "[File Group 0]\nNum Files=2\nFile 0=\"a\"\nFile 1=\"b\"\n[Package]\nFile Format=\"vip\"\n",
        );
        let names: Vec<String> = keys.into_iter().map(|(k, _)| k.1).collect();
        assert_eq!(names, ["Num Files", "File Format"]);
    }
}
