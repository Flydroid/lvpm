//! Steps of the documentation scenarios: the CLI's own `--help`, read the way a
//! user reads it, against the usage guide (docs/usage.md).
//!
//! The command tree is enumerated from the built binary's `-h` output, so the
//! test sees what the user sees and needs no access to the clap definitions.

use crate::CliWorld;
use cucumber::{then, when};
use std::path::Path;

/// The flags the lock workflow adds or relies on. Each must exist and be in the usage guide.
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

fn usage_guide() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/usage.md");
    let text = std::fs::read_to_string(&path).expect("read docs/usage.md");
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

#[then("every subcommand and every lock-related flag appears in the usage guide")]
async fn everything_is_in_the_readme(world: &mut CliWorld) {
    let guide = usage_guide();
    let undocumented: Vec<_> = world
        .docs
        .subcommands
        .iter()
        .filter(|item| !guide.contains(&format!("lvpm {}", item.name)))
        .map(|item| format!("lvpm {}", item.name))
        .collect();
    assert!(
        undocumented.is_empty(),
        "not in the usage guide: {undocumented:#?}"
    );
    for flag in LOCK_FLAGS {
        let in_the_tree = world
            .docs
            .flags
            .iter()
            .any(|item| item.name.ends_with(&format!(" {flag}")));
        assert!(in_the_tree, "{flag} is not a flag of any command");
        assert!(guide.contains(flag), "{flag} is not in the usage guide");
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

#[then(expr = "it names the manifest key {string}")]
async fn help_names_the_manifest_key(world: &mut CliWorld, key: String) {
    let help = world
        .output()
        .stdout
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let quoted = format!("manifest's `{key}`");
    assert!(help.contains(&quoted), "`{quoted}` is not in:\n{help}");
}

#[when("the usage guide's add section is read")]
async fn read_the_add_section(world: &mut CliWorld) {
    let text = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/usage.md"))
        .expect("read docs/usage.md");
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
    assert!(
        !section.is_empty(),
        "the usage guide has no section on adding"
    );
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

/// Every key `[distribution]` accepts.
const DISTRIBUTION_KEYS: &[&str] = &[
    "top-level-library",
    "kinds",
    "vendor",
    "display-name",
    "install-dir",
];

/// The usage guide's heading for the note on moving off `[distribution.vip]`.
const UPGRADE_HEADING: &str = "### Upgrading from [distribution.vip]";

fn repo_file(path: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(path))
        .unwrap_or_else(|why| panic!("read {path}: {why}"))
        .replace("\r\n", "\n")
}

/// The fenced `toml` blocks of `text`.
fn toml_blocks(text: &str) -> Vec<&str> {
    text.split("```toml\n")
        .skip(1)
        .map(|block| block.split("```").next().unwrap_or_default())
        .collect()
}

/// The keys at the start of lines of `[table]` in a TOML snippet.
fn keys_of_table(snippet: &str, table: &str) -> Vec<String> {
    let mut in_table = false;
    let mut keys = Vec::new();
    for line in snippet.lines() {
        if line.starts_with('[') {
            in_table = line.trim_end() == format!("[{table}]");
        } else if in_table && let Some((key, _)) = line.split_once('=') {
            keys.push(key.trim().to_string());
        }
    }
    keys
}

#[then(
    "docs/usage.md has a command-table line for lvpm build, names the .lvpkg output, and documents vendor, display-name and install-dir under [distribution]"
)]
async fn usage_documents_build(_: &mut CliWorld) {
    let guide = repo_file("docs/usage.md");
    let row = guide
        .lines()
        .find(|line| {
            line.split('|')
                .nth(1)
                .is_some_and(|cell| cell.trim() == "`lvpm build`")
        })
        .expect("no command-table line for lvpm build");
    assert!(
        row.contains(".lvpkg"),
        "the lvpm build row does not name .lvpkg: {row}"
    );
    let section = guide
        .split_once("## Building a package")
        .expect("usage.md has a Building a package section")
        .1;
    assert!(
        section.contains("dist/<name>-<version>.lvpkg"),
        "the Building section does not name the .lvpkg output"
    );
    // The first example, which comes before the upgrade note.
    let example = toml_blocks(section)[0];
    let documented = keys_of_table(example, "distribution");
    let missing: Vec<_> = DISTRIBUTION_KEYS
        .iter()
        .filter(|key| !documented.iter().any(|d| d == *key))
        .collect();
    assert!(
        missing.is_empty(),
        "[distribution] keys not documented: {missing:?} (found {documented:?})"
    );
}

#[then("docs/usage.md mentions [distribution.vip] only in its upgrade note")]
async fn usage_mentions_vip_only_in_the_upgrade_note(_: &mut CliWorld) {
    let guide = repo_file("docs/usage.md");
    let (before, rest) = guide
        .split_once(UPGRADE_HEADING)
        .expect("usage.md has no upgrade note");
    // The note runs to the next heading.
    let end = rest.find("\n#").map_or(rest.len(), |i| i + 1);
    let (note, after) = rest.split_at(end);
    for (name, text) in [("before", before), ("after", after)] {
        assert!(
            !text.contains("[distribution.vip"),
            "usage.md mentions [distribution.vip] {name} the upgrade note"
        );
    }
    let snippets = toml_blocks(note);
    assert_eq!(
        snippets.len(),
        2,
        "the upgrade note holds a before and an after"
    );
    assert!(
        snippets[0].contains("[distribution.vip]") && snippets[0].contains("formats"),
        "the before snippet does not show the old keys:\n{}",
        snippets[0]
    );
    assert!(
        !snippets[1].contains("distribution.vip") && !snippets[1].contains("formats"),
        "the after snippet still shows an old key:\n{}",
        snippets[1]
    );
    assert!(
        keys_of_table(snippets[1], "distribution").contains(&"vendor".to_string()),
        "the after snippet does not hold vendor under [distribution]"
    );
}

#[then("README.md shows lvpm build writing a .lvpkg")]
async fn readme_shows_build(_: &mut CliWorld) {
    let text = repo_file("README.md");
    assert!(
        text.lines()
            .any(|line| line.starts_with("$ lvpm build") && line.contains(".lvpkg")),
        "README.md does not show lvpm build writing a .lvpkg"
    );
}

#[then("ADR 1's example has no formats and no [distribution.vip]")]
async fn adr_1_example_is_current(_: &mut CliWorld) {
    let adr = repo_file("docs/adr/0001-manifest-project-and-distribution.md");
    let example = toml_blocks(&adr)
        .into_iter()
        .find(|block| block.contains("[distribution]\n"))
        .expect("ADR 1 has a [distribution] example");
    assert!(
        keys_of_table(example, "distribution").contains(&"vendor".to_string()),
        "ADR 1's example does not hold vendor under [distribution]:\n{example}"
    );
    for gone in ["formats", "[distribution.vip"] {
        assert!(
            !example.contains(gone),
            "ADR 1's example still has {gone}:\n{example}"
        );
    }
}

#[then(
    "no tracked file outside the history names the .vip writer, the VIPM probe or the S1b feature"
)]
async fn nothing_names_the_deleted_writer(_: &mut CliWorld) {
    // Built from parts, so that this file does not name them itself.
    let needles = [
        ["spec", "::", "write"].concat(),
        ["vipm", "_probe"].concat(),
        ["s1b-vip", "-in-vipm"].concat(),
    ];
    // The history: the specs, plans and ADRs that tell what happened, and the
    // tooling's own append-only logs.
    let history = ["docs/specs/", "plans/", "docs/adr/", "metrics/", ".claude/"];
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let listed = std::process::Command::new("git")
        .args(["ls-files", "-z"])
        .current_dir(root)
        .output()
        .expect("run git ls-files");
    assert!(listed.status.success(), "git ls-files failed");
    let listed = String::from_utf8(listed.stdout).expect("file names are UTF-8");
    let files: Vec<_> = listed.split('\0').filter(|f| !f.is_empty()).collect();
    assert!(
        files.contains(&"docs/usage.md"),
        "git ls-files does not list the repository"
    );
    let mut hits = Vec::new();
    for file in files
        .iter()
        .filter(|file| !history.iter().any(|h| file.starts_with(h)))
    {
        // A file the index lists but the work tree no longer has names nothing.
        let Ok(bytes) = std::fs::read(root.join(file)) else {
            continue;
        };
        let text = String::from_utf8_lossy(&bytes);
        hits.extend(
            needles
                .iter()
                .filter(|needle| text.contains(needle.as_str()))
                .map(|needle| format!("{file}: {needle}")),
        );
    }
    assert!(
        hits.is_empty(),
        "the deleted writer is still named:\n{}",
        hits.join("\n")
    );
}

#[then("docs/usage.md has a command-table line for lvpm publish")]
async fn usage_lists_publish(_: &mut CliWorld) {
    assert!(
        usage_guide().contains("| `lvpm publish --to <folder>` |"),
        "no command-table line for lvpm publish"
    );
}

#[then(
    regex = r#"^docs/usage\.md's section "([^"]+)" follows "([^"]+)" and holds "([^"]+)" and "([^"]+)"$"#
)]
async fn usage_section_follows(
    _: &mut CliWorld,
    section: String,
    after: String,
    a: String,
    b: String,
) {
    let guide = usage_guide();
    let (before, rest) = guide
        .split_once(&format!("## {section}"))
        .unwrap_or_else(|| panic!("usage.md has no {section} section"));
    assert!(
        before.contains(&format!("## {after}")),
        "{section} does not follow {after}"
    );
    let body = rest.split(" ## ").next().unwrap_or(rest);
    for want in [a, b] {
        assert!(body.contains(&want), "{section} does not say {want}");
    }
}

#[then("README.md shows lvpm publish")]
async fn readme_shows_publish(_: &mut CliWorld) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md");
    let text = std::fs::read_to_string(&path).expect("read README.md");
    assert!(
        text.contains("$ lvpm publish --to"),
        "README.md does not show lvpm publish"
    );
}
