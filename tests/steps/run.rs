//! Steps that run `lvpm` and keep what it did.

use crate::CliWorld;
use cucumber::when;

/// The `files` value that lists nothing.
const NO_FILES: &str = "-";
/// The `files` value that lists names without hashes.
const NAMES_ONLY_PREFIX: &str = "names:";

#[when(expr = "I run {string}")]
async fn run_command(world: &mut CliWorld, command_line: String) {
    world.run(&command_line, None);
}

#[when(expr = "I run {string} twice, the second time with --refresh")]
async fn run_twice(world: &mut CliWorld, command_line: String) {
    world.run(&command_line, None);
    world.first_output = world.last_output.take();
    world.run(&format!("{command_line} --refresh"), None);
}

#[when(expr = "I run {string} and answer {string}")]
async fn run_and_answer(world: &mut CliWorld, command_line: String, answer: String) {
    world.run(&command_line, Some(&format!("{answer}\n")));
}

/// `-` lists nothing; `names:<dir>` lists names only, for directories holding
/// files LabVIEW rewrites on every save; any other value lists names and hashes.
#[when(expr = "I list the files under {string}")]
pub(super) async fn list_files(world: &mut CliWorld, directory: String) {
    let listing = match directory.as_str() {
        NO_FILES => return,
        named => match named.strip_prefix(NAMES_ONLY_PREFIX) {
            Some(only) => world.sandbox.list_file_names(only),
            None => world.sandbox.list_files(named),
        },
    };
    world.last_output.as_mut().expect("a command was run").tree = Some(listing);
}
