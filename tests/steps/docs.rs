//! Steps of the documentation scenarios: the CLI's own `--help`, read the way a
//! user reads it, against the README.
//!
//! The command tree is enumerated from the built binary's `-h` output, so the
//! test sees what the user sees and needs no access to the clap definitions.

use crate::CliWorld;
use cucumber::{then, when};
use std::path::Path;

/// The flags the lock workflow adds or relies on. Each must exist and be in the README.
const LOCK_FLAGS: &[&str] = &[
    "--locked",
    "--frozen",
    "--no-locked",
    "--no-dev",
    "--dev",
    "--check",
    "--upgrade",
    "--upgrade-package",
];

/// A subcommand or a flag, as `-h` lists it.
#[derive(Debug)]
struct Item {
    /// `venv create`, or `lock --check`.
    name: String,
    has_help: bool,
}

/// What a documentation scenario has read.
#[derive(Debug, Default)]
pub struct DocsState {
    subcommands: Vec<Item>,
    flags: Vec<Item>,
    readme_section: String,
}

/// Names, each with whether the page says anything about it.
type Listed = Vec<(String, bool)>;

/// The items of one `-h` page: its subcommands and its flags, each with
/// whether it says anything. Items sit at an indent of 2 or 6; whatever is
/// indented further, under an item, is its help.
fn parse_help(page: &str) -> (Listed, Listed) {
    let (mut commands, mut flags): (Listed, Listed) = Default::default();
    let mut section = "";
    for line in page.lines() {
        let indent = line.len() - line.trim_start().len();
        let text = line.trim();
        if indent == 0 {
            section = text.trim_end_matches(':');
            continue;
        }
        let list = match section {
            "Commands" => &mut commands,
            "Options" | "Arguments" => &mut flags,
            _ => continue,
        };
        if indent > 6 {
            if let Some(last) = list.last_mut() {
                last.1 = true;
            }
            continue;
        }
        let (spec, help) = text.split_once("  ").unwrap_or((text, ""));
        let name = match section {
            "Commands" => spec.to_string(),
            _ => spec
                .split([' ', ','])
                .find(|word| word.starts_with("--"))
                .unwrap_or(spec)
                .to_string(),
        };
        list.push((name, !help.trim().is_empty()));
    }
    (commands, flags)
}

impl CliWorld {
    /// Read `lvpm <path> -h` and every subcommand page under it.
    fn walk_command_tree(&mut self, path: &str) {
        self.run(&format!("lvpm {path} -h"), None);
        assert_eq!(self.output().exit_code, 0, "{}", self.printed());
        let (commands, flags) = parse_help(&self.output().stdout);
        for (flag, has_help) in flags {
            let name = format!("{path} {flag}").trim().to_string();
            self.docs.flags.push(Item { name, has_help });
        }
        for (command, has_help) in commands.into_iter().filter(|(name, _)| name != "help") {
            let name = format!("{path} {command}").trim().to_string();
            self.docs.subcommands.push(Item {
                name: name.clone(),
                has_help,
            });
            self.walk_command_tree(&name);
        }
    }
}

fn readme() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md");
    let text = std::fs::read_to_string(&path).expect("read README.md");
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[when("the CLI's command tree is enumerated")]
async fn enumerate_the_command_tree(world: &mut CliWorld) {
    world.walk_command_tree("");
    // Not vacuous: the tree holds what this slice documents.
    let walked = |name: &str| world.docs.subcommands.iter().any(|item| item.name == name);
    assert!(
        walked("sync") && walked("venv create"),
        "the walk missed commands: {:?}",
        world.docs.subcommands
    );
}

#[then("every subcommand and every flag has a non-empty help text")]
async fn everything_has_help(world: &mut CliWorld) {
    let silent: Vec<_> = world
        .docs
        .subcommands
        .iter()
        .chain(&world.docs.flags)
        .filter(|item| !item.has_help)
        .map(|item| item.name.as_str())
        .collect();
    assert!(silent.is_empty(), "no help text: {silent:#?}");
}

#[then("every subcommand and every lock-related flag appears in the README")]
async fn everything_is_in_the_readme(world: &mut CliWorld) {
    let readme = readme();
    let undocumented: Vec<_> = world
        .docs
        .subcommands
        .iter()
        .filter(|item| !readme.contains(&format!("lvpm {}", item.name)))
        .map(|item| format!("lvpm {}", item.name))
        .collect();
    assert!(
        undocumented.is_empty(),
        "not in the README: {undocumented:#?}"
    );
    for flag in LOCK_FLAGS {
        let in_the_tree = world
            .docs
            .flags
            .iter()
            .any(|item| item.name.ends_with(&format!(" {flag}")));
        assert!(in_the_tree, "{flag} is not a flag of any command");
        assert!(readme.contains(flag), "{flag} is not in the README");
    }
}

#[then("it says that in a project bare install is sync and a named install is add")]
async fn install_help_says_what_it_does_in_a_project(world: &mut CliWorld) {
    let help = world
        .output()
        .stdout
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    for phrase in ["bare install is `lvpm sync`", "named install is `lvpm add`"] {
        assert!(help.contains(phrase), "`{phrase}` is not in:\n{help}");
    }
}

#[when("the README's add section is read")]
async fn read_the_add_section(world: &mut CliWorld) {
    let text = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md"))
        .expect("read README.md");
    let (mut in_fence, mut level, mut section) = (false, 0, String::new());
    for line in text.lines() {
        in_fence ^= line.starts_with("```");
        let heading_level = line.chars().take_while(|c| *c == '#').count();
        let is_heading = !in_fence && heading_level > 0 && line[heading_level..].starts_with(' ');
        if is_heading && level > 0 && heading_level <= level {
            break;
        }
        if is_heading && level == 0 && line[heading_level..].trim_start().starts_with("Adding") {
            level = heading_level;
        }
        if level > 0 {
            section.push_str(line);
            section.push('\n');
        }
    }
    assert!(!section.is_empty(), "the README has no section on adding");
    world.docs.readme_section = section;
}

#[then(expr = "it shows a quoted example {string} for PowerShell and cmd")]
async fn the_section_shows_the_quoted_example(world: &mut CliWorld, example: String) {
    let section = &world.docs.readme_section;
    // The feature file escapes the quotes inside the quoted string.
    let example = example.replace("\\\"", "\"");
    assert!(
        section.contains(&example),
        "`{example}` is not in:\n{section}"
    );
    for shell in ["PowerShell", "cmd"] {
        assert!(
            section.contains(shell),
            "{shell} is not named in:\n{section}"
        );
    }
}
