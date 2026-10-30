//! The git repository `lvpm init` makes. This module is the only place that
//! builds a git command; the caller hands in how a command is run.

use serde::Serialize;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// How a command is run: [`spawn`] for real, a fake in a test.
pub type Runner<'a> = &'a dyn Fn(&mut Command) -> io::Result<Output>;

/// The real runner. Its output is captured, so no git text reaches the terminal.
pub fn spawn(command: &mut Command) -> io::Result<Output> {
    command.output()
}

/// What [`init`] did.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GitOutcome {
    /// `git init` made a repository.
    Initialized,
    /// The folder is already in a work tree, whose top folder is `top`.
    InsideWorkTree { top: PathBuf },
    /// There is no `git` to run.
    GitMissing,
    /// `git` ran and failed, or could not be started: `why` is its own text.
    Failed { why: String },
}

/// The folder at or above `root` that holds a `.git`, a directory or a file
/// (a worktree or submodule). Nothing is spawned, so a repository git refuses
/// for its ownership, or one that is broken, still counts as one to leave alone.
pub fn inside_work_tree(root: &Path) -> Option<PathBuf> {
    root.ancestors()
        .find(|dir| dir.join(".git").exists())
        .map(Path::to_path_buf)
}

/// `git init` in `root`, unless it is in a work tree already.
pub fn init(root: &Path, run: Runner) -> GitOutcome {
    if let Some(top) = inside_work_tree(root) {
        return GitOutcome::InsideWorkTree { top };
    }
    let mut command = Command::new("git");
    command.arg("init").current_dir(root);
    match run(&mut command) {
        Ok(output) if output.status.success() => GitOutcome::Initialized,
        Ok(output) => GitOutcome::Failed {
            why: failure_text(&output),
        },
        Err(error) if error.kind() == ErrorKind::NotFound => GitOutcome::GitMissing,
        Err(error) => GitOutcome::Failed {
            why: error.to_string(),
        },
    }
}

/// What git said on stderr, else how it exited.
fn failure_text(output: &Output) -> String {
    let said = String::from_utf8_lossy(&output.stderr);
    match said.trim() {
        "" => output.status.to_string(),
        text => text.to_string(),
    }
}

/// What a fake runner answers: git exited with `code` and said `stderr`.
#[cfg(test)]
pub(crate) fn fake_output(code: i32, stderr: &str) -> io::Result<Output> {
    #[cfg(windows)]
    let status = std::os::windows::process::ExitStatusExt::from_raw(code as u32);
    #[cfg(unix)]
    let status = std::os::unix::process::ExitStatusExt::from_raw(code << 8);
    Ok(Output {
        status,
        stdout: Vec::new(),
        stderr: stderr.as_bytes().to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::ffi::OsString;

    /// A temp folder, removed on drop, with `below` made inside it.
    struct Tree(PathBuf);

    impl Tree {
        fn new(tag: &str, below: &str) -> (Tree, PathBuf) {
            let top =
                std::env::temp_dir().join(format!("lvpm-git-test-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&top);
            let deep = top.join(below);
            std::fs::create_dir_all(&deep).unwrap();
            (Tree(top), deep)
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// What a fake runner was asked to run: program, arguments, folder.
    type Call = (OsString, Vec<OsString>, Option<PathBuf>);

    /// `init` on `root` with a runner that answers `answer` and keeps its calls.
    fn run_init(root: &Path, answer: impl Fn() -> io::Result<Output>) -> (GitOutcome, Vec<Call>) {
        let calls = RefCell::new(Vec::new());
        let outcome = init(root, &|command| {
            calls.borrow_mut().push((
                command.get_program().to_owned(),
                command.get_args().map(OsString::from).collect(),
                command.get_current_dir().map(Path::to_path_buf),
            ));
            answer()
        });
        (outcome, calls.into_inner())
    }

    #[test]
    fn a_git_directory_two_levels_up_is_found() {
        let (tree, deep) = Tree::new("dir-up", "a/b/c");
        let top = tree.0.join("a");
        std::fs::create_dir(top.join(".git")).unwrap();

        assert_eq!(inside_work_tree(&deep), Some(top));
    }

    #[test]
    fn a_git_file_above_is_found_too() {
        let (tree, deep) = Tree::new("file-up", "a/b/c");
        let top = tree.0.join("a");
        std::fs::write(top.join(".git"), "gitdir: elsewhere\n").unwrap();

        assert_eq!(inside_work_tree(&deep), Some(top));
    }

    #[test]
    fn the_root_itself_holding_a_git_directory_is_its_own_top() {
        let (_tree, deep) = Tree::new("self", "a");
        std::fs::create_dir(deep.join(".git")).unwrap();

        assert_eq!(inside_work_tree(&deep), Some(deep));
    }

    #[test]
    fn a_tree_with_no_git_has_no_work_tree_at_or_above_it() {
        let (tree, deep) = Tree::new("none", "a/b");

        // The temp directory itself is outside any repository on a sound machine.
        assert_eq!(
            inside_work_tree(&deep).filter(|top| top.starts_with(&tree.0)),
            None
        );
    }

    #[test]
    fn git_init_runs_in_the_root_and_exit_zero_is_initialized() {
        let (_tree, root) = Tree::new("ok", "a");

        let (outcome, calls) = run_init(&root, || fake_output(0, ""));

        assert_eq!(outcome, GitOutcome::Initialized);
        assert_eq!(
            calls,
            [("git".into(), vec!["init".into()], Some(root.clone()))]
        );
    }

    #[test]
    fn git_not_found_is_git_missing() {
        let (_tree, root) = Tree::new("missing", "a");

        let (outcome, calls) = run_init(&root, || Err(io::Error::from(ErrorKind::NotFound)));

        assert_eq!(outcome, GitOutcome::GitMissing);
        assert_eq!(calls.len(), 1);
    }

    #[test]
    fn a_failing_git_is_failed_with_its_stderr() {
        let (_tree, root) = Tree::new("fails", "a");

        let (outcome, _) = run_init(&root, || fake_output(128, "fatal: permission denied\n"));

        assert_eq!(
            outcome,
            GitOutcome::Failed {
                why: "fatal: permission denied".into()
            }
        );
    }

    #[test]
    fn a_failing_git_with_nothing_to_say_is_failed_with_its_exit_status() {
        let (_tree, root) = Tree::new("fails-quiet", "a");

        let (outcome, _) = run_init(&root, || fake_output(128, ""));

        let GitOutcome::Failed { why } = outcome else {
            panic!("{outcome:?}")
        };
        assert!(why.contains("128"), "{why}");
    }

    #[test]
    fn a_spawn_error_other_than_not_found_is_failed_with_its_text() {
        let (_tree, root) = Tree::new("denied", "a");

        let (outcome, _) = run_init(&root, || {
            Err(io::Error::new(ErrorKind::PermissionDenied, "access denied"))
        });

        assert_eq!(
            outcome,
            GitOutcome::Failed {
                why: "access denied".into()
            }
        );
    }

    #[test]
    fn inside_a_work_tree_git_init_is_never_run() {
        let (tree, root) = Tree::new("inside", "repo/app");
        let top = tree.0.join("repo");
        std::fs::create_dir(top.join(".git")).unwrap();

        let (outcome, calls) = run_init(&root, || panic!("the runner must not be called"));

        assert_eq!(outcome, GitOutcome::InsideWorkTree { top });
        assert!(calls.is_empty());
    }
}
