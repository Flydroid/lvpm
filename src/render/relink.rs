//! The wording of the relink events.

use super::{Renderer, Stream};
use lvpm_core::events::Event;

pub(super) fn render(r: &mut Renderer, event: &Event) {
    match event {
        Event::RelinkDoneNotClearable { indicator, error } => r.line(
            Stream::Err,
            &format!(
                "      note: cannot clear {indicator} ({error}) — relying on the VI to clear it"
            ),
        ),
        Event::RelinkProgress { elapsed, value } => r.line(
            Stream::Out,
            &format!("        {:>6.0}s  {value}", elapsed.as_secs_f64()),
        ),
        Event::RelinkProgressUnreadable { indicator, error } => r.line(
            Stream::Err,
            &format!("      note: cannot read {indicator} ({error}) — not reporting progress"),
        ),
        Event::RelinkStarted {
            folders,
            covered_by_parent,
            vi,
        } => {
            let covered = match covered_by_parent {
                0 => String::new(),
                n => format!("  ({n} covered by a parent)"),
            };
            r.line(
                Stream::Out,
                &format!(
                    "\nrelinking {folders} folder(s) with {}{covered}",
                    vi.display()
                ),
            );
        }
        Event::RelinkFolderStarted { packages, folder } => r.begin(
            Stream::Out,
            &format!("  {}\n      {} ... ", packages.join(", "), folder.display()),
        ),
        Event::RelinkFolderDone { took, summary } => {
            r.end(Stream::Out, &format!("{:.1}s", took.as_secs_f64()));
            for line in summary {
                r.line(Stream::Out, &format!("        {line}"));
            }
        }
        Event::RelinkFolderFailed { error } => {
            r.end(Stream::Out, "FAILED");
            r.line(Stream::Out, &format!("      {error}"));
        }
        Event::RelinkPlanned { folders, packages } => {
            r.line(
                Stream::Out,
                &format!(
                    "\nrelink: {folders} folder(s) across {} package(s):",
                    packages.len()
                ),
            );
            for package in packages {
                for folder in &package.folders {
                    r.line(
                        Stream::Out,
                        &format!("      {}: {}", package.package, folder.display()),
                    );
                }
            }
        }
        Event::VenvLabviewLeftOpen { port } => r.line(
            Stream::Out,
            &format!("\nleaving the venv LabVIEW open (--keep-open), VI Server on port {port}"),
        ),
        Event::NothingToRelink { package, version } => r.line(
            Stream::Out,
            &format!("{package} {version} installed no VIs, libraries or LLBs — nothing to relink"),
        ),
        _ => unreachable!("routed here by Renderer::render: {event:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::renderer;
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    #[test]
    fn progress_is_a_right_aligned_second_count_and_the_value_on_stdout() {
        let (mut renderer, out, err) = renderer();
        renderer.render(&Event::RelinkProgress {
            elapsed: Duration::from_secs(12),
            value: "Walking vi.lib".into(),
        });
        assert_eq!(out.text(), "            12s  Walking vi.lib\n");
        assert_eq!(err.text(), "");
    }

    #[test]
    fn a_folder_is_a_line_begun_before_the_relink_and_ended_with_its_time() {
        let (mut renderer, out, _) = renderer();
        renderer.render(&Event::RelinkStarted {
            folders: 2,
            covered_by_parent: 1,
            vi: PathBuf::from("relink.vi"),
        });
        renderer.render(&Event::RelinkFolderStarted {
            packages: vec!["a".into(), "b".into()],
            folder: PathBuf::from("vi.lib"),
        });
        renderer.render(&Event::RelinkFolderDone {
            took: Duration::from_millis(1_240),
            summary: vec!["saved 3".into()],
        });
        renderer.render(&Event::RelinkFolderStarted {
            packages: vec!["c".into()],
            folder: PathBuf::from("menus"),
        });
        renderer.render(&Event::RelinkFolderFailed {
            error: "timed out".into(),
        });
        assert_eq!(
            out.text(),
            "\nrelinking 2 folder(s) with relink.vi  (1 covered by a parent)\n  a, b\n      vi.lib ... 1.2s\n        saved 3\n  c\n      menus ... FAILED\n      timed out\n"
        );
    }

    #[test]
    fn a_planned_pass_lists_each_folder_under_its_package() {
        use lvpm_core::events::PackageFolders;
        let (mut renderer, out, _) = renderer();
        renderer.render(&Event::RelinkPlanned {
            folders: 2,
            packages: vec![PackageFolders {
                package: "alpha".into(),
                folders: vec![PathBuf::from("x"), PathBuf::from("y")],
            }],
        });
        renderer.render(&Event::VenvLabviewLeftOpen { port: 3400 });
        assert_eq!(
            out.text(),
            "\nrelink: 2 folder(s) across 1 package(s):\n      alpha: x\n      alpha: y\n\nleaving the venv LabVIEW open (--keep-open), VI Server on port 3400\n"
        );
    }

    #[test]
    fn a_package_with_nothing_to_relink_is_a_line_on_stdout() {
        let (mut renderer, out, err) = renderer();
        renderer.render(&Event::NothingToRelink {
            package: "docs".into(),
            version: "1.0".into(),
        });
        assert_eq!(
            out.text(),
            "docs 1.0 installed no VIs, libraries or LLBs — nothing to relink\n"
        );
        assert_eq!(err.text(), "");
    }

    #[test]
    fn the_notes_go_to_stderr() {
        let (mut renderer, out, err) = renderer();
        renderer.render(&Event::RelinkDoneNotClearable {
            indicator: "Done".into(),
            error: "error 1055".into(),
        });
        renderer.render(&Event::RelinkProgressUnreadable {
            indicator: "Status".into(),
            error: "no such control".into(),
        });
        assert_eq!(
            err.text(),
            "      note: cannot clear Done (error 1055) — relying on the VI to clear it\n\
             \x20     note: cannot read Status (no such control) — not reporting progress\n"
        );
        assert_eq!(out.text(), "");
    }
}
