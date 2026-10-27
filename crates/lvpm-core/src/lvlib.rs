//! A LabVIEW library's members, read from its `.lvlib` (and every `.lvclass`
//! and nested `.lvlib` in it) without LabVIEW.
//!
//! A member is an `<Item … URL="…">`. The URL is relative to the library file
//! itself, as if the file were a folder: `../A.vi` is a sibling of the
//! `.lvlib`, and a bare `Logger.ctl` lives inside a `.lvclass`.

use anyhow::{Context, Result, bail};
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

/// Every file the library at `lib` is made of, `lib` itself first. Nested
/// libraries and classes contribute themselves and their members; an
/// auto-populating folder contributes every file in it on disk. A listed
/// member missing on disk is an error that names it.
pub fn members(lib: &Path) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut seen = HashSet::new();
    collect(lib, &mut found, &mut seen)?;
    Ok(found)
}

fn collect(file: &Path, found: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>) -> Result<()> {
    if !seen.insert(file.to_path_buf()) {
        return Ok(());
    }
    found.push(file.to_path_buf());
    let text = std::fs::read_to_string(file)
        .with_context(|| format!("{} is not a readable library", file.display()))?;
    if !text.contains("<Library") && !text.contains("<LVClass") {
        bail!(
            "{} is not a readable library: it holds no <Library> or <LVClass>",
            file.display()
        );
    }
    for item in items(&text).with_context(|| format!("reading {}", file.display()))? {
        let (Some(url), ty) = (attr(&item, "URL"), attr(&item, "Type").unwrap_or_default()) else {
            continue; // a virtual folder: its items follow as tags of their own
        };
        if ty == "Class Private Data" {
            continue; // stored inside the .lvclass, not a file
        }
        let path =
            resolve(file, &url).with_context(|| format!("{} lists {url:?}", file.display()))?;
        if ty == "Folder" {
            folder_files(&path, file, found, seen)?;
            continue;
        }
        if !path.is_file() {
            bail!(
                "{} lists {}, which is not on disk",
                file.display(),
                path.display()
            );
        }
        match ext(&path).as_str() {
            "lvlib" | "lvclass" => collect(&path, found, seen)?,
            _ => {
                if seen.insert(path.clone()) {
                    found.push(path);
                }
            }
        }
    }
    Ok(())
}

/// An auto-populating folder: every file below it on disk, in name order.
fn folder_files(
    dir: &Path,
    owner: &Path,
    found: &mut Vec<PathBuf>,
    seen: &mut HashSet<PathBuf>,
) -> Result<()> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| {
            format!(
                "{} lists the folder {}, which is not on disk",
                owner.display(),
                dir.display()
            )
        })?
        .map(|e| e.map(|e| e.path()))
        .collect::<std::io::Result<_>>()?;
    entries.sort();
    for path in entries {
        // A link could lead out of the library or round in a loop.
        if std::fs::symlink_metadata(&path)?.file_type().is_symlink() {
            continue;
        }
        if path.is_dir() {
            folder_files(&path, owner, found, seen)?;
        } else if matches!(ext(&path).as_str(), "lvlib" | "lvclass") {
            collect(&path, found, seen)?;
        } else if seen.insert(path.clone()) {
            found.push(path);
        }
    }
    Ok(())
}

fn ext(p: &Path) -> String {
    p.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

/// `url` against the library file `base`, taken as a folder.
fn resolve(base: &Path, url: &str) -> Result<PathBuf> {
    if url.starts_with('/') || url.contains(':') || url.contains('\\') {
        bail!("an absolute or symbolic path is not a member lvpm can package");
    }
    let mut path = base.to_path_buf();
    for part in url.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                path.pop();
            }
            p => path.push(p),
        }
    }
    Ok(path)
}

// shortcut: a scanner over `<Item` start tags, not an XML parser — it trusts
// that `<Item` appears only as a tag (not in comments or CDATA), which holds
// for the files LabVIEW writes; use an XML crate if a library ever breaks it.
/// The attribute text of every `<Item` start tag, entities still encoded.
fn items(text: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("<Item") {
        let after = &rest[at + "<Item".len()..];
        if !after.starts_with(|c: char| c.is_whitespace() || c == '>' || c == '/') {
            rest = after; // `<Items…`, some other tag
            continue;
        }
        let mut quote = None;
        let end = after.char_indices().find_map(|(i, c)| match (quote, c) {
            (None, '"' | '\'') => {
                quote = Some(c);
                None
            }
            (Some(q), c) if c == q => {
                quote = None;
                None
            }
            (None, '>') => Some(i),
            _ => None,
        });
        let Some(end) = end else {
            bail!("an <Item tag is not closed");
        };
        out.push(after[..end].trim_end_matches('/').to_string());
        rest = &after[end..];
    }
    Ok(out)
}

/// The decoded value of attribute `name` in a tag's attribute text.
fn attr(tag: &str, name: &str) -> Option<String> {
    let mut rest = tag;
    loop {
        let eq = rest.find('=')?;
        let key = rest[..eq].trim();
        let value = rest[eq + 1..].trim_start();
        let q = value.chars().next().filter(|c| *c == '"' || *c == '\'')?;
        let close = value[1..].find(q)?;
        if key == name {
            return Some(decode(&value[1..1 + close]));
        }
        rest = &value[close + 2..];
    }
}

fn decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let Some(semi) = tail.find(';') else {
            out.push_str(tail);
            return out;
        };
        let entity = &tail[1..semi];
        let ch = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            e => e
                .strip_prefix("#x")
                .map(|h| u32::from_str_radix(h, 16))
                .or_else(|| e.strip_prefix('#').map(str::parse))
                .and_then(|n| n.ok())
                .and_then(char::from_u32),
        };
        match ch {
            Some(c) => out.push(c),
            None => out.push_str(&tail[..=semi]),
        }
        rest = &tail[semi + 1..];
    }
    out.push_str(rest);
    out
}

/// `path` lexically, with `.` and `..` folded away — for comparing a member
/// against the folder it must live in.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lvpm-lvlib-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn write(dir: &Path, rel: &str, text: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn rels(dir: &Path, found: Vec<PathBuf>) -> Vec<String> {
        found
            .iter()
            .map(|p| {
                p.strip_prefix(dir)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect()
    }

    const LIB: &str = r#"<?xml version='1.0' encoding='UTF-8'?>
<Library LVVersion="16008000">
	<Property Name="NI.Lib.Version" Type="Str">1.0.0.0</Property>
	<Item Name="Folder" Type="Folder">
		<Item Name="A &amp; B.vi" URL="../A &amp; B.vi" Type="VI"/>
	</Item>
	<Item Type="LVClass" Name="C.lvclass" URL="../C/C.lvclass"/>
	<Item Name="T.ctl" Type="VI" URL='../T.ctl'></Item>
</Library>
"#;

    const CLASS: &str = r#"<?xml version='1.0' encoding='UTF-8'?>
<LVClass LVVersion="16008000">
	<Item Name="C.ctl" Type="Class Private Data" URL="C.ctl">
		<Property Name="NI.LibItem.Scope" Type="Int">2</Property>
	</Item>
	<Item Name="M.vi" Type="VI" URL="../M.vi">
		<Property Name="NI.LibItem.Scope" Type="Int">1</Property>
	</Item>
</LVClass>
"#;

    #[test]
    fn members_follow_urls_in_any_attribute_order_and_recurse_into_classes() {
        let d = &scratch("1");
        write(d, "Lib.lvlib", LIB);
        write(d, "A & B.vi", "");
        write(d, "T.ctl", "");
        write(d, "C/C.lvclass", CLASS);
        write(d, "C/M.vi", "");
        write(d, "Stray.vi", "");
        let found = members(&d.join("Lib.lvlib")).unwrap();
        assert_eq!(
            rels(d, found),
            ["Lib.lvlib", "A & B.vi", "C/C.lvclass", "C/M.vi", "T.ctl"]
        );
    }

    #[test]
    fn an_auto_populating_folder_contributes_its_files_on_disk() {
        let d = &scratch("2");
        write(
            d,
            "Lib.lvlib",
            r#"<Library><Item Name="Data" Type="Folder" URL="../Data">
<Item Name="d.txt" Type="Document" URL="../Data/d.txt"/></Item></Library>"#,
        );
        write(d, "Data/d.txt", "");
        write(d, "Data/sub/e.txt", "");
        let found = members(&d.join("Lib.lvlib")).unwrap();
        assert_eq!(
            rels(d, found),
            ["Lib.lvlib", "Data/d.txt", "Data/sub/e.txt"]
        );
    }

    #[test]
    fn a_missing_member_an_unclosed_tag_and_a_non_library_fail_naming_the_file() {
        let d = &scratch("3");
        write(
            d,
            "Lib.lvlib",
            r#"<Library><Item Name="G" URL="../Gone.vi"/></Library>"#,
        );
        let e = format!("{:#}", members(&d.join("Lib.lvlib")).unwrap_err());
        assert!(e.contains("Gone.vi") && e.contains("not on disk"), "{e}");

        write(
            d,
            "Lib.lvlib",
            r#"<Library><Item Name="G" URL="../Gone.vi""#,
        );
        let e = format!("{:#}", members(&d.join("Lib.lvlib")).unwrap_err());
        assert!(e.contains("not closed"), "{e}");

        write(d, "Lib.lvlib", "not xml at all");
        let e = format!("{:#}", members(&d.join("Lib.lvlib")).unwrap_err());
        assert!(
            e.contains("Lib.lvlib") && e.contains("not a readable library"),
            "{e}"
        );
    }

    #[test]
    fn entities_decode_and_a_url_leaving_the_folder_resolves_outside_it() {
        assert_eq!(decode("a&amp;b&lt;&#65;&#x42;&bogus;"), "a&b<AB&bogus;");
        let p = resolve(Path::new("x/Lib.lvlib"), "../../Outside.vi").unwrap();
        assert_eq!(p, Path::new("Outside.vi"));
        assert!(resolve(Path::new("x/Lib.lvlib"), "/<vilib>/a.vi").is_err());
        assert_eq!(normalize(Path::new("a/./b/../c")), Path::new("a/c"));
    }
}
