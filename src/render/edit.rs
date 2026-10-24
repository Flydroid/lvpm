//! The wording of `lvpm add` and `lvpm remove`: what they did to `lvpm.toml`,
//! and the errors they end in. The lock and the sync they run say what they do
//! in their own words (`lock`, `sync`).

use super::{Renderer, Stream};
use lvpm_core::events::{Event, ManifestEdit};

pub(super) fn render(r: &mut Renderer, event: &Event) {
    match event {
        Event::ManifestEdited { edit, dev, dry_run } => {
            r.line(Stream::Out, &edit_line(edit, *dev, *dry_run))
        }
        Event::PackageStillRequired { name, required_by } => r.line(
            Stream::Out,
            &format!(
                "{name} is still required by {}: it stays in lvpm.lock and installed",
                required_by.join(", ")
            ),
        ),
        _ => unreachable!("routed here by Renderer::render: {event:?}"),
    }
}

/// What happened to one manifest entry, or on a dry run what would.
fn edit_line(edit: &ManifestEdit, dev: bool, dry_run: bool) -> String {
    let table = if dev { " [dev-dependencies]" } else { "" };
    match (edit, dry_run) {
        (ManifestEdit::Added { name, constraint }, false) => {
            format!(
                "lvpm.toml: added {name} = \"{constraint}\" to{}",
                table_or(dev)
            )
        }
        (ManifestEdit::Added { name, constraint }, true) => {
            format!("lvpm.toml would get: {name} = \"{constraint}\"{table}")
        }
        (ManifestEdit::Updated { name, from, to }, false) => {
            format!("lvpm.toml was updated: {name} \"{from}\" -> \"{to}\"{table}")
        }
        (ManifestEdit::Updated { name, from, to }, true) => {
            format!("lvpm.toml would be updated: {name} \"{from}\" -> \"{to}\"{table}")
        }
        (ManifestEdit::Removed { name, constraint }, false) => {
            format!("lvpm.toml: removed {name} = \"{constraint}\"{table}")
        }
        (ManifestEdit::Removed { name, constraint }, true) => {
            format!("lvpm.toml would lose: {name} = \"{constraint}\"{table}")
        }
    }
}

fn table_or(dev: bool) -> &'static str {
    if dev {
        " [dev-dependencies]"
    } else {
        " [dependencies]"
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::renderer;
    use super::*;

    fn added() -> ManifestEdit {
        ManifestEdit::Added {
            name: "gamma".into(),
            constraint: ">=1.0".into(),
        }
    }

    #[test]
    fn an_edit_is_a_line_on_stdout_and_a_dry_run_says_it_is_only_a_plan() {
        let (mut renderer, out, err) = renderer();
        for (edit, dev, dry_run) in [
            (added(), false, false),
            (added(), true, false),
            (added(), false, true),
            (
                ManifestEdit::Updated {
                    name: "beta".into(),
                    from: ">=1.0".into(),
                    to: ">=0.5".into(),
                },
                false,
                false,
            ),
            (
                ManifestEdit::Removed {
                    name: "gamma".into(),
                    constraint: ">=1.0".into(),
                },
                true,
                true,
            ),
        ] {
            renderer.render(&Event::ManifestEdited { edit, dev, dry_run });
        }
        assert_eq!(
            out.text(),
            "lvpm.toml: added gamma = \">=1.0\" to [dependencies]\n\
             lvpm.toml: added gamma = \">=1.0\" to [dev-dependencies]\n\
             lvpm.toml would get: gamma = \">=1.0\"\n\
             lvpm.toml was updated: beta \">=1.0\" -> \">=0.5\"\n\
             lvpm.toml would lose: gamma = \">=1.0\" [dev-dependencies]\n"
        );
        assert_eq!(err.text(), "");
    }

    #[test]
    fn a_package_that_stays_says_which_packages_require_it() {
        let (mut renderer, out, _) = renderer();
        renderer.render(&Event::PackageStillRequired {
            name: "alpha".into(),
            required_by: vec!["beta".into(), "caraya".into()],
        });
        assert_eq!(
            out.text(),
            "alpha is still required by beta, caraya: it stays in lvpm.lock and installed\n"
        );
    }
}
