//! The preview of an install: what its dry run said it would do, package by
//! package, folded from core's events.

use lvpm_core::events::Event;

/// What an install does to one package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    New,
    Upgrade { from: String },
    Downgrade { from: String },
    Already,
}

impl Action {
    pub fn label(&self) -> &'static str {
        match self {
            Action::New => "new",
            Action::Upgrade { .. } => "upgrade",
            Action::Downgrade { .. } => "downgrade",
            Action::Already => "already installed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub name: String,
    pub version: String,
    pub action: Action,
    /// The files it writes.
    pub files: usize,
}

#[derive(Debug, Clone, Default)]
pub struct Preview {
    /// The packages in the order core installs them: dependencies first.
    pub items: Vec<Item>,
    /// Dependencies no source has; the install goes on without them.
    pub missing: Vec<String>,
    /// The package being replaced, met before its new version starts.
    replacing: Option<(String, Action)>,
}

impl Preview {
    /// Take in one event of the dry run.
    pub fn add(&mut self, event: &Event) {
        match event {
            Event::PackageAlreadyInstalled { name, version } => self.items.push(Item {
                name: name.clone(),
                version: version.clone(),
                action: Action::Already,
                files: 0,
            }),
            Event::PackageReplacing {
                name,
                installed,
                upgrade,
                ..
            } => {
                let from = installed.clone();
                let action = match upgrade {
                    true => Action::Upgrade { from },
                    false => Action::Downgrade { from },
                };
                self.replacing = Some((name.clone(), action));
            }
            Event::PackageStart { name, version, .. } => {
                let action = match self.replacing.take() {
                    Some((replaced, action)) if &replaced == name => action,
                    _ => Action::New,
                };
                self.items.push(Item {
                    name: name.clone(),
                    version: version.clone(),
                    action,
                    files: 0,
                });
            }
            Event::PackageDone { files, .. } => {
                if let Some(item) = self.items.last_mut() {
                    item.files = *files;
                }
            }
            Event::DependencyUnresolved { name } => self.missing.push(name.clone()),
            _ => {}
        }
    }

    /// The package the install would downgrade, if any.
    pub fn downgrade(&self) -> Option<&Item> {
        let mut items = self.items.iter();
        items.find(|item| matches!(item.action, Action::Downgrade { .. }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start(name: &str, version: &str) -> Event {
        Event::PackageStart {
            name: name.into(),
            version: version.into(),
            dry_run: true,
        }
    }

    fn replacing(name: &str, installed: &str, upgrade: bool) -> Event {
        Event::PackageReplacing {
            name: name.into(),
            installed: installed.into(),
            wanted: "2.0".into(),
            upgrade,
            old_files: 1,
            dry_run: true,
        }
    }

    fn preview(events: &[Event]) -> Preview {
        let mut preview = Preview::default();
        events.iter().for_each(|event| preview.add(event));
        preview
    }

    #[test]
    fn each_package_is_new_upgraded_downgraded_or_left_with_its_files() {
        let preview = preview(&[
            Event::PackageAlreadyInstalled {
                name: "gamma".into(),
                version: "1.0".into(),
            },
            start("beta", "1.0"),
            Event::PackageDone {
                files: 3,
                dry_run: true,
            },
            replacing("alpha", "1.0", true),
            start("alpha", "2.0"),
            replacing("delta", "3.0", false),
            start("delta", "2.0"),
        ]);
        let actions: Vec<_> = preview.items.iter().map(|i| i.action.clone()).collect();
        assert_eq!(
            actions,
            [
                Action::Already,
                Action::New,
                Action::Upgrade { from: "1.0".into() },
                Action::Downgrade { from: "3.0".into() },
            ]
        );
        assert_eq!(preview.items[1].files, 3);
        assert_eq!(preview.downgrade().map(|i| i.name.as_str()), Some("delta"));
    }

    #[test]
    fn a_replacement_counts_only_for_the_package_it_names() {
        let preview = preview(&[replacing("alpha", "1.0", true), start("beta", "1.0")]);
        assert_eq!(preview.items[0].action, Action::New);
        assert_eq!(preview.downgrade(), None);
    }
}
