//! Steps that run `lvpm` and keep what it did.

use crate::CliWorld;
use cucumber::when;

/// The `files` value that lists nothing.
const NO_FILES: &str = "-";

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

#[when(expr = "I list the files under {string}")]
async fn list_files(world: &mut CliWorld, directory: String) {
    if directory == NO_FILES {
        return;
    }
    let listing = world.sandbox.list_files(&directory);
    world.last_output.as_mut().expect("a command was run").tree = Some(listing);
}
