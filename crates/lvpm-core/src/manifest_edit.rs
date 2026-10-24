//! Edits to one entry of `lvpm.toml`, leaving every other byte as it was.
//!
//! The manifest is user-owned (comments, ordering, blank lines), so an edit
//! goes through `toml_edit`, which keeps the formatting of what it does not
//! touch. [`set`] and [`remove`] are pure: text in, text out. [`set_file`] and
//! [`remove_file`] apply one to a file, and write only when the text changed.

use anyhow::{Context, Result};
use std::path::Path;
use toml_edit::{DocumentMut, Item, Table, Value};

/// Which dependency table an edit targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyTable {
    Dependencies,
    DevDependencies,
}

impl DependencyTable {
    fn key(self) -> &'static str {
        match self {
            DependencyTable::Dependencies => "dependencies",
            DependencyTable::DevDependencies => "dev-dependencies",
        }
    }
}

/// `text` with `name = "<constraint>"` in `table`: a new entry is added at the
/// end of the table (the table is appended to the file when it is missing), an
/// existing one changes its value only, keeping its same-line comment.
pub fn set(text: &str, table: DependencyTable, name: &str, constraint: &str) -> Result<String> {
    let mut doc = parse(text)?;
    let key = table.key();
    if doc.get(key).is_none() {
        return Ok(with_appended_table(text, key, name, constraint));
    }
    let entries = doc
        .get_mut(key)
        .and_then(Item::as_table_like_mut)
        .with_context(|| format!("[{key}] is not a table"))?;
    match entries.get_mut(name) {
        Some(Item::Value(old)) if old.as_str() == Some(constraint) => return Ok(text.to_string()),
        Some(Item::Value(old)) => {
            let mut new = Value::from(constraint);
            *new.decor_mut() = old.decor().clone();
            *old = new;
        }
        Some(other) => *other = Item::Value(constraint.into()),
        None => {
            entries.insert(name, Item::Value(constraint.into()));
        }
    }
    Ok(render(text, &doc))
}

/// `text` without the `name` entry of `table`: its own line, with any
/// same-line comment, and nothing else. Comment lines and blank lines above it
/// stay. The table header stays too, even when that was its last entry. Text
/// without such an entry comes back as it was.
pub fn remove(text: &str, table: DependencyTable, name: &str) -> Result<String> {
    let doc = toml_edit::Document::parse(text).map_err(not_valid)?;
    let key = table.key();
    let Some(item) = doc.get(key) else {
        return Ok(text.to_string());
    };
    let entries = item
        .as_table_like()
        .with_context(|| format!("[{key}] is not a table"))?;
    let Some((entry_key, entry)) = entries.get_key_value(name) else {
        return Ok(text.to_string());
    };
    let line = entry_key
        .span()
        .zip(entry.span())
        .and_then(|(key_span, value_span)| own_line(text, key_span.start, value_span.end));
    match line {
        Some(line) => Ok(format!("{}{}", &text[..line.start], &text[line.end..])),
        None => remove_through_document(text, key, name),
    }
}

/// The whole lines holding an entry that starts at `start` and ends at `end`,
/// when it is alone on them, apart from a comment after it. `None` for an
/// entry sharing its line with something else, such as one in an inline table.
fn own_line(text: &str, start: usize, end: usize) -> Option<std::ops::Range<usize>> {
    let line_start = text[..start].rfind('\n').map_or(0, |at| at + 1);
    let line_end = text[end..].find('\n').map_or(text.len(), |at| end + at + 1);
    let before = &text[line_start..start];
    let after = text[end..line_end].trim_start_matches([' ', '\t']);
    let alone = before.chars().all(|c| c == ' ' || c == '\t')
        && (after.is_empty() || after.starts_with(['#', '\r', '\n']));
    alone.then_some(line_start..line_end)
}

/// The edit for an entry that is not on a line of its own: the document drops
/// it, comments attached to it included.
fn remove_through_document(text: &str, table: &str, name: &str) -> Result<String> {
    let mut doc = parse(text)?;
    if let Some(entries) = doc.get_mut(table).and_then(Item::as_table_like_mut) {
        entries.remove(name);
    }
    Ok(render(text, &doc))
}

/// [`set`] on the manifest at `path`. The file is written only if it changed.
pub fn set_file(path: &Path, table: DependencyTable, name: &str, constraint: &str) -> Result<()> {
    edit_file(path, |text| set(text, table, name, constraint))
}

/// [`remove`] on the manifest at `path`. The file is written only if it changed.
pub fn remove_file(path: &Path, table: DependencyTable, name: &str) -> Result<()> {
    edit_file(path, |text| remove(text, table, name))
}

fn edit_file(path: &Path, edit: impl FnOnce(&str) -> Result<String>) -> Result<()> {
    let before = std::fs::read_to_string(path)
        .with_context(|| format!("reading manifest {}", path.display()))?;
    let after = edit(&before).with_context(|| format!("editing manifest {}", path.display()))?;
    if after != before {
        std::fs::write(path, after)
            .with_context(|| format!("writing manifest {}", path.display()))?;
    }
    Ok(())
}

fn parse(text: &str) -> Result<DocumentMut> {
    text.parse::<DocumentMut>().map_err(not_valid)
}

/// The parse error names the line and column and quotes the line.
fn not_valid(why: toml_edit::TomlError) -> anyhow::Error {
    anyhow::anyhow!("lvpm.toml is not valid TOML, so it was not edited: {why}")
}

/// `text` followed by a new `[key]` table holding the one entry. Built by
/// appending, not through the document, so that every byte of `text` stays
/// exactly where it was (toml_edit would move a trailing blank line below the
/// new table). The table is set off by a blank line unless one is there.
fn with_appended_table(text: &str, key: &str, name: &str, constraint: &str) -> String {
    let mut entries = Table::new();
    entries.insert(name, Item::Value(constraint.into()));
    let mut fragment = DocumentMut::new();
    fragment.insert(key, Item::Table(entries));

    let eol = line_ending(text);
    let mut out = text.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push_str(eol);
    }
    if !out.is_empty() && !out.ends_with("\n\n") && !out.ends_with("\r\n\r\n") {
        out.push_str(eol);
    }
    out.push_str(&fragment.to_string().replace('\n', eol));
    out
}

/// The document as text. New lines use the file's own line ending: a file
/// whose every line ends in CRLF stays CRLF; anything else is left as it is.
fn render(original: &str, doc: &DocumentMut) -> String {
    let text = doc.to_string();
    match uses_crlf_throughout(original) {
        true => text.replace("\r\n", "\n").replace('\n', "\r\n"),
        false => text,
    }
}

fn uses_crlf_throughout(text: &str) -> bool {
    let crlf = text.matches("\r\n").count();
    crlf > 0 && crlf == text.matches('\n').count()
}

fn line_ending(text: &str) -> &'static str {
    match uses_crlf_throughout(text) {
        true => "\r\n",
        false => "\n",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use DependencyTable::{Dependencies, DevDependencies};

    const MANIFEST: &str = "\
# a header comment

[project]
name = \"fixture\"

[sources]
mirror = \"http://host/files\"
# the last word on sources

[dependencies]
alpha = \"1.0\"
beta = \">=1.0\"
";

    #[test]
    fn a_new_entry_is_one_added_line_at_the_end_of_its_table() {
        let after = set(MANIFEST, Dependencies, "gamma", ">=1.0").unwrap();
        assert_eq!(after, format!("{MANIFEST}gamma = \">=1.0\"\n"));
    }

    #[test]
    fn a_new_entry_goes_before_the_tables_that_follow() {
        let text = "[dependencies]\nalpha = \"1.0\"\n\n[nipm.dependencies]\nx = \"1\"\n";
        let after = set(text, Dependencies, "gamma", "*").unwrap();
        assert_eq!(
            after,
            "[dependencies]\nalpha = \"1.0\"\ngamma = \"*\"\n\n[nipm.dependencies]\nx = \"1\"\n"
        );
    }

    #[test]
    fn replacing_changes_the_value_and_keeps_the_comment_and_the_key_spelling() {
        let text = "[dependencies]\n\"alpha\" = '1.0'  # pinned for DQMH\n";
        let after = set(text, Dependencies, "alpha", ">=2.0").unwrap();
        assert_eq!(
            after,
            "[dependencies]\n\"alpha\" = \">=2.0\"  # pinned for DQMH\n"
        );
    }

    #[test]
    fn removing_takes_the_line_and_keeps_the_header_of_an_emptied_table() {
        let after = remove(MANIFEST, Dependencies, "alpha").unwrap();
        assert_eq!(after, MANIFEST.replace("alpha = \"1.0\"\n", ""));
        let text = "[dependencies]\nalpha = \"1.0\"\n";
        assert_eq!(
            remove(text, Dependencies, "alpha").unwrap(),
            "[dependencies]\n"
        );
    }

    const COMMENTED: &str = "[dependencies]\n# the first\nalpha = \"1.0\"  # same line\n\n# the middle\nbeta = \">=1.0\"\n# the last\ngamma = \"*\"\n";

    #[test]
    fn removing_a_first_entry_keeps_the_comment_lines_above_it() {
        assert_eq!(
            remove(COMMENTED, Dependencies, "alpha").unwrap(),
            "[dependencies]\n# the first\n\n# the middle\nbeta = \">=1.0\"\n# the last\ngamma = \"*\"\n"
        );
    }

    #[test]
    fn removing_a_middle_entry_keeps_the_comment_lines_above_it() {
        assert_eq!(
            remove(COMMENTED, Dependencies, "beta").unwrap(),
            "[dependencies]\n# the first\nalpha = \"1.0\"  # same line\n\n# the middle\n# the last\ngamma = \"*\"\n"
        );
    }

    #[test]
    fn removing_a_last_entry_keeps_the_comment_lines_above_it() {
        assert_eq!(
            remove(COMMENTED, Dependencies, "gamma").unwrap(),
            "[dependencies]\n# the first\nalpha = \"1.0\"  # same line\n\n# the middle\nbeta = \">=1.0\"\n# the last\n"
        );
    }

    #[test]
    fn removing_an_entry_keeps_comment_lines_in_a_crlf_file() {
        let crlf = COMMENTED.replace('\n', "\r\n");
        assert_eq!(
            remove(&crlf, Dependencies, "beta").unwrap(),
            "[dependencies]\n# the first\nalpha = \"1.0\"  # same line\n\n# the middle\n# the last\ngamma = \"*\"\n"
                .replace('\n', "\r\n")
        );
        assert_eq!(
            remove(&crlf, Dependencies, "gamma").unwrap(),
            COMMENTED
                .replace("gamma = \"*\"\n", "")
                .replace('\n', "\r\n")
        );
    }

    #[test]
    fn removing_the_last_entry_without_a_final_newline_works() {
        assert_eq!(
            remove(
                "[dependencies]\n# c\nalpha = \"1.0\"",
                Dependencies,
                "alpha"
            )
            .unwrap(),
            "[dependencies]\n# c\n"
        );
    }

    #[test]
    fn an_entry_in_an_inline_table_is_removed_by_the_document() {
        assert_eq!(
            remove(
                "dependencies = { alpha = \"1.0\", beta = \"*\" }\n",
                Dependencies,
                "alpha"
            )
            .unwrap(),
            "dependencies = { beta = \"*\" }\n"
        );
    }

    #[test]
    fn removing_what_is_not_there_changes_nothing() {
        assert_eq!(remove(MANIFEST, Dependencies, "nope").unwrap(), MANIFEST);
        assert_eq!(
            remove(MANIFEST, DevDependencies, "alpha").unwrap(),
            MANIFEST
        );
    }

    #[test]
    fn a_missing_table_is_appended_after_a_blank_line() {
        let after = set(MANIFEST, DevDependencies, "caraya", ">=1.0").unwrap();
        assert_eq!(
            after,
            format!("{MANIFEST}\n[dev-dependencies]\ncaraya = \">=1.0\"\n")
        );
        let blank = format!("{MANIFEST}\n");
        assert_eq!(
            set(&blank, DevDependencies, "caraya", "*").unwrap(),
            format!("{blank}[dev-dependencies]\ncaraya = \"*\"\n")
        );
        assert_eq!(
            set("", Dependencies, "a", "*").unwrap(),
            "[dependencies]\na = \"*\"\n"
        );
    }

    #[test]
    fn a_file_without_a_final_newline_gets_one_before_the_new_table() {
        let after = set("[project]\nname = \"x\"", DevDependencies, "c", "*").unwrap();
        assert_eq!(
            after,
            "[project]\nname = \"x\"\n\n[dev-dependencies]\nc = \"*\"\n"
        );
    }

    #[test]
    fn an_equal_value_comes_back_untouched_whatever_its_quoting() {
        let text = "[dependencies]\nalpha = '1.0'   # x\n";
        assert_eq!(set(text, Dependencies, "alpha", "1.0").unwrap(), text);
    }

    #[test]
    fn crlf_files_stay_crlf_for_every_edit() {
        let crlf = MANIFEST.replace('\n', "\r\n");
        for after in [
            set(&crlf, Dependencies, "gamma", "*").unwrap(),
            set(&crlf, DevDependencies, "caraya", "*").unwrap(),
            remove(&crlf, Dependencies, "alpha").unwrap(),
        ] {
            assert_eq!(after.matches('\n').count(), after.matches("\r\n").count());
        }
        assert_eq!(
            set(&crlf, Dependencies, "gamma", "*").unwrap(),
            format!("{crlf}gamma = \"*\"\r\n")
        );
    }

    #[test]
    fn invalid_toml_is_refused_naming_the_line() {
        let text = "[project]\nname = \"x\"\n\n[dependencies]\nalpha 1.0\n";
        let why = format!("{:#}", set(text, Dependencies, "gamma", "*").unwrap_err());
        assert!(why.contains("line 5"), "{why}");
        assert!(why.contains("alpha 1.0"), "{why}");
        assert!(remove(text, Dependencies, "alpha").is_err());
    }

    #[test]
    fn a_dependencies_value_that_is_not_a_table_is_an_error() {
        assert!(set("dependencies = 3\n", Dependencies, "a", "*").is_err());
        assert!(remove("dependencies = 3\n", Dependencies, "a").is_err());
    }

    #[test]
    fn files_are_written_only_when_the_text_changed() {
        let dir = std::env::temp_dir().join(format!("lvpm-manifest-edit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("lvpm.toml");
        std::fs::write(&path, MANIFEST).unwrap();
        let long_ago =
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(long_ago)
            .unwrap();

        set_file(&path, Dependencies, "alpha", "1.0").unwrap();
        remove_file(&path, Dependencies, "nope").unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            long_ago
        );

        set_file(&path, Dependencies, "alpha", "2.0").unwrap();
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("alpha = \"2.0\"")
        );
        assert_ne!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            long_ago
        );

        remove_file(&path, Dependencies, "alpha").unwrap();
        assert!(!std::fs::read_to_string(&path).unwrap().contains("alpha"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
