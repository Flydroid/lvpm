//! The findings table: one line per variant, and where lvpm's build differs
//! from the published archive. Written under `target/`, never committed.

use crate::variants::RefDiff;

/// What the table's header names.
pub struct Header {
    pub vipm: String,
    pub labview: String,
}

/// One variant's outcome.
#[derive(Debug)]
pub struct Row {
    pub variant: String,
    pub accepted: bool,
    pub exit: Option<i32>,
    /// The first line of VIPM's output, or why the probe rejected it.
    pub note: String,
    /// Whether an accepted variant installed the files VIPM's install of the
    /// published archive did; `None` when it was not installed.
    pub same_files: Option<bool>,
}

/// A cell on one line: a pipe would end it, a newline would end its row, and
/// an angle bracket could open markup.
pub fn escape(cell: &str) -> String {
    cell.replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace('<', "\\<")
        .replace('>', "\\>")
        .replace("\r\n", "\\n")
        .replace(['\n', '\r'], "\\n")
}

fn list(title: &str, items: &[String]) -> String {
    let mut out = format!("\n### {title}\n\n");
    match items.is_empty() {
        true => out.push_str("none\n"),
        false => items
            .iter()
            .for_each(|item| out.push_str(&format!("- {}\n", escape(item)))),
    }
    out
}

pub fn render(header: &Header, rows: &[Row], diff: &RefDiff, not_varied: &[String]) -> String {
    let mut out = format!(
        "# VIPM probe\n\nVIPM: {}\nLabVIEW: {}\n\n\
         \"accepted\" at a version VIPM already has is its cached copy, not this file (ADR 6).\n\n\
         | variant | verdict | exit | first line of VIPM's output | same files as reference |\n\
         |---|---|---|---|---|\n",
        escape(&header.vipm),
        escape(&header.labview)
    );
    for row in rows {
        let verdict = if row.accepted { "accepted" } else { "rejected" };
        let exit = row.exit.map_or("-".to_string(), |code| code.to_string());
        let same = match row.same_files {
            Some(true) => "yes",
            Some(false) => "no",
            None => "-",
        };
        out.push_str(&format!(
            "| {} | {verdict} | {exit} | {} | {same} |\n",
            escape(&row.variant),
            escape(&row.note)
        ));
    }
    out.push_str("\n## lvpm's build of pilot A against the published archive\n");
    out.push_str(&list("Entries only in lvpm's build", &diff.only_ours));
    out.push_str(&list(
        "Entries only in the published archive",
        &diff.only_reference,
    ));
    out.push_str(&list(
        "Spec keys only in lvpm's build",
        &diff.keys_only_ours,
    ));
    out.push_str(&list(
        "Spec keys only in the published archive",
        &diff.keys_only_reference,
    ));
    let changed: Vec<String> = diff
        .keys_changed
        .iter()
        .map(|(key, ours, theirs)| format!("{key}: lvpm {ours}, published {theirs}"))
        .collect();
    out.push_str(&list("Spec keys whose value differs", &changed));
    out.push_str(&list(
        "Keys in every published spec that lvpm does not write and no variant sets",
        not_varied,
    ));
    out
}

/// `<n> variants: <a> accepted (...), <r> rejected; table: <path>`
pub fn summary_line(rows: &[Row], table: &std::path::Path) -> String {
    let accepted = rows.iter().filter(|row| row.accepted).count();
    format!(
        "{} variants: {accepted} accepted (a cached copy if VIPM knew the version), {} rejected; table: {}",
        rows.len(),
        rows.len() - accepted,
        table.display()
    )
}

/// `YYYYMMDDTHHMMSS` of a time given as seconds since the Unix epoch, in UTC.
pub fn utc_stamp(seconds: u64) -> String {
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    // Days since 1970-01-01 to a calendar date (Howard Hinnant's civil_from_days).
    let z = days + 719_468;
    let era = z / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}{:02}",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// The folder a run writes to: `<date>T<time>-vipm-<version>`, the version
/// being the first dotted number of `vipm --version`'s first line.
pub fn run_folder(stamp: &str, vipm_version: &str) -> String {
    let token = vipm_version
        .split_whitespace()
        .find(|t| t.contains('.') && t.chars().any(|c| c.is_ascii_digit()))
        .unwrap_or(vipm_version);
    let safe: String = token
        .chars()
        .map(|c| match c.is_ascii_alphanumeric() || c == '.' {
            true => c,
            false => '_',
        })
        .collect();
    format!("{stamp}-vipm-{safe}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_summary_counts_accepted_and_rejected() {
        let row = |accepted| Row {
            variant: "v".into(),
            accepted,
            exit: None,
            note: String::new(),
            same_files: None,
        };
        let rows = [row(true), row(false), row(true)];
        let line = summary_line(&rows, std::path::Path::new("t/table.md"));
        assert_eq!(
            line,
            "3 variants: 2 accepted (a cached copy if VIPM knew the version), 1 rejected; table: t/table.md"
        );
    }

    #[test]
    fn a_stamp_is_the_utc_date_and_time() {
        assert_eq!(utc_stamp(0), "19700101T000000");
        assert_eq!(utc_stamp(1_700_000_000), "20231114T221320");
        assert_eq!(utc_stamp(1_709_164_799), "20240228T235959");
        assert_eq!(utc_stamp(1_709_164_800), "20240229T000000");
    }

    #[test]
    fn the_run_folder_names_the_vipm_version() {
        assert_eq!(
            run_folder("20240229T000000", "Example CLI 1.2.3 (build 9)"),
            "20240229T000000-vipm-1.2.3"
        );
        assert_eq!(run_folder("s", "odd/name"), "s-vipm-odd_name");
    }

    fn header() -> Header {
        Header {
            vipm: "vipm 1.2.3".into(),
            labview: "26.3.0".into(),
        }
    }

    fn row(variant: &str, accepted: bool, exit: Option<i32>, note: &str) -> Row {
        Row {
            variant: variant.into(),
            accepted,
            exit,
            note: note.into(),
            same_files: None,
        }
    }

    fn lines_of_rows(text: &str) -> Vec<&str> {
        text.lines()
            .filter(|l| l.starts_with("| "))
            .skip(1) // the header line
            .collect()
    }

    #[test]
    fn table_has_one_line_per_variant() {
        let mut base = row("base", true, Some(0), "ok");
        base.same_files = Some(true);
        let mut empty_id = row("ID=empty", true, Some(0), "fine");
        empty_id.same_files = Some(false);
        let rows = [
            base,
            empty_id,
            row("icon.bmp=present", false, None, "timed out after 600s"),
        ];
        let text = render(&header(), &rows, &RefDiff::default(), &[]);
        let lines = lines_of_rows(&text);
        assert_eq!(lines.len(), 3, "{text}");
        assert_eq!(lines[0], "| base | accepted | 0 | ok | yes |");
        assert_eq!(lines[1], "| ID=empty | accepted | 0 | fine | no |");
        assert_eq!(
            lines[2],
            "| icon.bmp=present | rejected | - | timed out after 600s | - |"
        );
        assert!(text.contains("VIPM: vipm 1.2.3") && text.contains("LabVIEW: 26.3.0"));
    }

    #[test]
    fn table_escapes_pipes_and_newlines() {
        let rows = [row("a|b", false, Some(1), "x | y\nz\r\nw")];
        let text = render(&header(), &rows, &RefDiff::default(), &[]);
        assert_eq!(
            lines_of_rows(&text),
            ["| a\\|b | rejected | 1 | x \\| y\\nz\\nw | - |"]
        );
    }

    #[test]
    fn a_backslash_is_escaped_so_it_cannot_cancel_the_escape_of_a_pipe() {
        assert_eq!(escape("a\\|b"), "a\\\\\\|b");
    }

    #[test]
    fn angle_brackets_cannot_open_markup_in_a_cell() {
        assert_eq!(escape("<b>x</b> a<b"), "\\<b\\>x\\</b\\> a\\<b");
        let rows = [row("v", false, Some(1), "<script>")];
        let text = render(&header(), &rows, &RefDiff::default(), &["<x>".into()]);
        assert!(text.contains("| \\<script\\> |"), "{text}");
        assert!(text.contains("- \\<x\\>"), "{text}");
    }

    #[test]
    fn the_reference_diff_is_in_the_table() {
        let diff = RefDiff {
            only_ours: vec!["a.txt".into()],
            only_reference: vec!["icon.bmp".into()],
            keys_only_ours: vec!["[Package]/X".into()],
            keys_only_reference: vec!["[Package]/ID".into()],
            keys_changed: vec![("[Package]/Version".into(), "1".into(), "2".into())],
        };
        let text = render(&header(), &[], &diff, &["[LabVIEW]/y".into()]);
        assert_eq!(
            after_the_rows(&text),
            "\
## lvpm's build of pilot A against the published archive

### Entries only in lvpm's build

- a.txt

### Entries only in the published archive

- icon.bmp

### Spec keys only in lvpm's build

- [Package]/X

### Spec keys only in the published archive

- [Package]/ID

### Spec keys whose value differs

- [Package]/Version: lvpm 1, published 2

### Keys in every published spec that lvpm does not write and no variant sets

- [LabVIEW]/y
"
        );
    }

    #[test]
    fn an_empty_reference_diff_says_none_in_every_section() {
        let text = render(&header(), &[], &RefDiff::default(), &[]);
        assert_eq!(
            after_the_rows(&text),
            "\
## lvpm's build of pilot A against the published archive

### Entries only in lvpm's build

none

### Entries only in the published archive

none

### Spec keys only in lvpm's build

none

### Spec keys only in the published archive

none

### Spec keys whose value differs

none

### Keys in every published spec that lvpm does not write and no variant sets

none
"
        );
    }

    /// The table from the reference diff's heading to its end.
    fn after_the_rows(text: &str) -> &str {
        &text[text.find("## lvpm's build").expect("the diff heading")..]
    }
}
