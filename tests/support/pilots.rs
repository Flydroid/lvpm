//! The pilot libraries of the build slices: their archives, each with a SHA-256 checksum
//! in `tests/fixtures/pilots/SHA256SUMS`, and what a mass compile of them says.

use super::snapshot::hex;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The checksum file in the pilot folder: one SHA-256 per archive.
pub const CHECKSUMS_FILE: &str = "SHA256SUMS";
const SHA256_HEX_LEN: usize = 64;

/// Where the pilot archives are committed.
pub fn pilots_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pilots")
}

/// The `.vip` files directly in `dir`, by name.
pub fn archives(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .map(|entry| entry.expect("a directory entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "vip"))
        .collect();
    found.sort();
    found
}

/// The checksums of `SHA256SUMS` text: `<sha256>  <file>` per line, or `<sha256> *<file>`.
pub fn parse_checksums(text: &str) -> Result<Vec<(String, String)>, String> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| match line.split_once(' ') {
            Some((sha, rest)) if sha.len() == SHA256_HEX_LEN => {
                let name = rest.strip_prefix([' ', '*']).unwrap_or(rest);
                Ok((sha.to_lowercase(), name.to_string()))
            }
            _ => Err(format!("not a `<sha256>  <file>` line: {line:?}")),
        })
        .collect()
}

/// Check every archive in `dir` against its `SHA256SUMS`: each listed file
/// exists with that SHA-256, and no archive goes unlisted.
pub fn check_checksums(dir: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(dir.join(CHECKSUMS_FILE))
        .map_err(|e| format!("reading {CHECKSUMS_FILE} in {}: {e}", dir.display()))?;
    let checksums = parse_checksums(&text)?;
    for (expected, name) in &checksums {
        let bytes = std::fs::read(dir.join(name)).map_err(|e| format!("{name}: {e}"))?;
        let actual = hex(&Sha256::digest(&bytes));
        if &actual != expected {
            return Err(format!("{name}: SHA-256 is {actual}, expected {expected}"));
        }
    }
    for archive in archives(dir) {
        let name = archive.file_name().expect("a file name").to_string_lossy();
        if !checksums.iter().any(|(_, listed)| *listed == name) {
            return Err(format!("{name} has no checksum in {CHECKSUMS_FILE}"));
        }
    }
    Ok(())
}

/// The `.lvlib` file names inside a `.vip`: what would show that its package
/// is already installed somewhere.
pub fn library_names(archive: &Path) -> Vec<String> {
    let file = std::fs::File::open(archive)
        .unwrap_or_else(|e| panic!("opening {}: {e}", archive.display()));
    let mut zip = zip::ZipArchive::new(file)
        .unwrap_or_else(|e| panic!("reading {} as a zip: {e}", archive.display()));
    let mut names: Vec<String> = (0..zip.len())
        .filter_map(|i| {
            let entry = zip.by_index(i).expect("a zip entry");
            let name = entry.name().rsplit(['/', '\\']).next()?.to_string();
            name.to_lowercase().ends_with(".lvlib").then_some(name)
        })
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Every file below `root` whose name is one of `names` (ignoring case).
pub fn find_files_named(root: &Path, names: &[String]) -> Vec<PathBuf> {
    let wanted: Vec<String> = names.iter().map(|name| name.to_lowercase()).collect();
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => pending.push(path),
                Ok(_) if wanted.contains(&entry.file_name().to_string_lossy().to_lowercase()) => {
                    found.push(path)
                }
                _ => {}
            }
        }
    }
    found
}

/// The lines of a LabVIEW mass-compile log that report a VI it could not
/// compile or load; each names the VI.
pub fn bad_vi_lines(log: &str) -> Vec<String> {
    log.lines()
        .filter(|line| {
            let line = line.to_lowercase();
            line.contains("bad vi") || line.contains("failed to load")
        })
        .map(|line| line.trim().to_string())
        .collect()
}

/// LabVIEWCLI's default location; `PATH` is tried first.
const LABVIEWCLI_DEFAULT: &str =
    r"C:\Program Files (x86)\National Instruments\Shared\LabVIEW CLI\LabVIEWCLI.exe";

/// The LabVIEWCLI to run: the one on `PATH`, else NI's default location.
pub fn labviewcli() -> PathBuf {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .map(|dir| dir.join("LabVIEWCLI.exe"))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| PathBuf::from(LABVIEWCLI_DEFAULT))
}

/// Pilot A's pinned commit; README.md beside the build-slice features names it.
pub const PILOT_A_COMMIT: &str = "f7cc16a391faa1e982b54ae22d2f43c9df1ef581";
/// Pilot A as its vendor published it.
pub const PILOT_A_ARCHIVE: &str = "hse_lib_hse_logger-2.2.2.114.vip";
/// What `lvpm build` names pilot A's package.
pub const PILOT_A_BUILT: &str = "hse_lib_hse_logger-2.2.2.114.lvpkg";
/// The folder of pilot A's checkout that its build copies.
pub const PILOT_A_SUBTREE: &str = "Source";
/// Pilot A's `lvpm.toml`: its repository has none, and this is what it would hold.
pub const PILOT_A_MANIFEST: &str = r#"[project]
name = "hse_lib_hse_logger"
version = "2.2.2.114"
labview = ">=2016"

[distribution]
top-level-library = "Source/HSE-Logger/hse-logger.lvlib"
vendor = "Hampel Software Engineering"
display-name = "HSE Logger"
"#;

/// The checkout the variable `env_var` names, once its HEAD is `commit` and
/// nothing in its `subtree` (the folder a build copies) is modified or
/// untracked; otherwise what is wrong with it.
pub fn pilot_checkout(env_var: &str, commit: &str, subtree: &str) -> Result<PathBuf, String> {
    let path = std::env::var_os(env_var).ok_or_else(|| format!("{env_var} names no checkout"))?;
    checkout_at(env_var, path, commit, subtree)
}

/// `git -C path`, with no repository chosen by the caller's environment.
pub(crate) fn git_command(path: &Path) -> Command {
    let mut git = Command::new("git");
    git.arg("-C").arg(path);
    for var in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"] {
        git.env_remove(var);
    }
    // git's untranslated messages, so errors and tests read the same on any locale
    git.env("LC_ALL", "C").env("LANGUAGE", "C");
    git
}

fn checkout_at(
    env_var: &str,
    path: std::ffi::OsString,
    commit: &str,
    subtree: &str,
) -> Result<PathBuf, String> {
    let path = PathBuf::from(path);
    let shown = path.display();
    let at = format!("{env_var}={shown}");
    if !path.is_dir() {
        return Err(format!("{at} does not exist or is not a folder"));
    }
    // The checkout itself must be the repository, not a folder inside one.
    // git ignores a relative ceiling, so the path is made absolute first.
    let path = std::path::absolute(&path).map_err(|e| format!("{at}: its full path: {e}"))?;
    let ceiling = path.parent().unwrap_or(&path);
    let git = |args: &[&str]| {
        let output = git_command(&path)
            .env("GIT_CEILING_DIRECTORIES", ceiling)
            .args(args)
            .output()
            .map_err(|e| format!("{at}: running git: {e}"))?;
        match output.status.success() {
            true => Ok(String::from_utf8_lossy(&output.stdout).trim().to_string()),
            false => Err(format!(
                "{at}: `git {}` failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            )),
        }
    };
    let head = git(&["rev-parse", "HEAD"])?;
    if head != commit {
        return Err(format!(
            "{at} is at {head}, not the pinned commit {commit}; run `git -C \"{shown}\" checkout {commit}`"
        ));
    }
    let status = [
        "--no-optional-locks",
        "status",
        "--porcelain",
        "--untracked-files=all",
        // a git-ignored file in the subtree is copied by the build too
        "--ignored",
        "--",
        subtree,
    ];
    if !git(&status)?.is_empty() {
        return Err(format!(
            "{at} has uncommitted changes in {subtree} (`git -C \"{shown}\" status -- {subtree}` lists them); build from a clean checkout"
        ));
    }
    Ok(path)
}

/// The file entries of a package below `File Group 0/<under>`, relative to
/// `File Group 0/`, sorted.
pub fn group_zero_files(archive: &Path, under: &str) -> Vec<String> {
    let file = std::fs::File::open(archive)
        .unwrap_or_else(|e| panic!("opening {}: {e}", archive.display()));
    let zip = zip::ZipArchive::new(file)
        .unwrap_or_else(|e| panic!("reading {} as a zip: {e}", archive.display()));
    let mut files: Vec<String> = zip
        .file_names()
        .filter_map(|name| name.strip_prefix("File Group 0/"))
        .filter(|name| name.starts_with(under) && !name.ends_with('/'))
        .map(String::from)
        .collect();
    files.sort();
    files
}

/// The payload of a `.lvpkg`: the raw names of its zip entries but `RECORD`
/// and `package.toml`, each below its `install-dir`, sorted. Read without
/// `lvpkg::read`, which is the code under test.
pub fn lvpkg_files(archive: &Path) -> Vec<String> {
    let file = std::fs::File::open(archive)
        .unwrap_or_else(|e| panic!("opening {}: {e}", archive.display()));
    let zip = zip::ZipArchive::new(file)
        .unwrap_or_else(|e| panic!("reading {} as a zip: {e}", archive.display()));
    let mut files: Vec<String> = zip
        .file_names()
        .filter(|name| *name != "RECORD" && *name != "package.toml")
        .map(String::from)
        .collect();
    files.sort();
    files
}

/// Copy the tree below `from` to `to`.
pub fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap_or_else(|e| panic!("creating {}: {e}", to.display()));
    for entry in std::fs::read_dir(from).unwrap_or_else(|e| panic!("{}: {e}", from.display())) {
        let path = entry.expect("a directory entry").path();
        let target = to.join(path.file_name().expect("a file name"));
        match path.is_dir() {
            true => copy_tree(&path, &target),
            false => drop(std::fs::copy(&path, &target).expect("copy a file")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA_OF_ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    fn folder_with(content: &[u8], checksums: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a-1.0.vip"), content).unwrap();
        std::fs::write(dir.path().join(CHECKSUMS_FILE), checksums).unwrap();
        dir
    }

    #[test]
    fn the_committed_pilots_match_their_checksums() {
        assert_eq!(check_checksums(&pilots_dir()), Ok(()));
    }

    #[test]
    fn an_archive_matching_its_checksum_passes() {
        let dir = folder_with(b"abc", &format!("{SHA_OF_ABC}  a-1.0.vip\n"));
        assert_eq!(check_checksums(dir.path()), Ok(()));
    }

    #[test]
    fn a_changed_archive_fails_naming_it() {
        let dir = folder_with(b"abd", &format!("{SHA_OF_ABC}  a-1.0.vip\n"));
        let why = check_checksums(dir.path()).unwrap_err();
        assert!(why.starts_with("a-1.0.vip: SHA-256 is "), "{why}");
    }

    #[test]
    fn an_unlisted_archive_fails_naming_it() {
        let dir = folder_with(b"abc", &format!("{SHA_OF_ABC}  a-1.0.vip\n"));
        std::fs::write(dir.path().join("b-1.0.vip"), b"b").unwrap();
        assert_eq!(
            check_checksums(dir.path()),
            Err(format!("b-1.0.vip has no checksum in {CHECKSUMS_FILE}"))
        );
    }

    #[test]
    fn a_checksum_line_without_a_sha256_is_refused() {
        assert!(parse_checksums("abc  a-1.0.vip\n").is_err());
    }

    #[test]
    fn a_binary_mode_checksum_line_is_read() {
        assert_eq!(
            parse_checksums(&format!("{SHA_OF_ABC} *a-1.0.vip\n")),
            Ok(vec![(SHA_OF_ABC.to_string(), "a-1.0.vip".to_string())])
        );
    }

    #[test]
    fn the_bad_lines_of_a_mass_compile_log_name_their_vis() {
        let log = "#### Starting Mass Compile\n  Directory: \"C:\\x\"\n\
                   ### Bad VI:    \"Broken.vi\"\t\tPath=\"C:\\x\\Broken.vi\"\n\
                   ### Failed to load:  \"Missing.vi\"\n#### Finished Mass Compile\n";
        let bad = bad_vi_lines(log);
        assert_eq!(bad.len(), 2, "{bad:?}");
        assert!(bad[0].contains("Broken.vi") && bad[1].contains("Missing.vi"));
    }

    #[test]
    fn a_clean_mass_compile_log_has_no_bad_lines() {
        assert!(
            bad_vi_lines("#### Starting Mass Compile\n#### Finished Mass Compile\n").is_empty()
        );
    }

    #[test]
    fn pilot_a_ships_its_library() {
        let archive = pilots_dir().join(PILOT_A_ARCHIVE);
        assert!(library_names(&archive).contains(&"hse-logger.lvlib".to_string()));
    }

    const VAR: &str = "LVPM_TEST_PILOT_X";
    /// The folder of the repositories below that a build would copy.
    const SUBTREE: &str = "Source";

    fn git(dir: &Path, args: &[&str]) -> String {
        let null_device = if cfg!(windows) { "NUL" } else { "/dev/null" };
        let output = git_command(dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", null_device)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .output()
            .expect("run git");
        assert!(output.status.success(), "git {args:?}: {output:?}");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    /// A repository with one commit, and that commit's id.
    fn repo_with_a_commit() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        std::fs::create_dir(dir.path().join(SUBTREE)).unwrap();
        std::fs::write(dir.path().join(SUBTREE).join("a.txt"), "a").unwrap();
        std::fs::write(dir.path().join("outside.txt"), "o").unwrap();
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "a"]);
        let head = git(dir.path(), &["rev-parse", "HEAD"]);
        (dir, head)
    }

    #[test]
    fn a_clean_checkout_at_the_pinned_commit_is_accepted() {
        let (dir, head) = repo_with_a_commit();
        assert_eq!(
            checkout_at(VAR, dir.path().as_os_str().to_owned(), &head, SUBTREE),
            Ok(dir.path().to_path_buf())
        );
    }

    #[test]
    fn a_checkout_at_another_commit_names_both_and_the_fix() {
        let (dir, first) = repo_with_a_commit();
        std::fs::write(dir.path().join("b.txt"), "b").unwrap();
        git(dir.path(), &["add", "b.txt"]);
        git(dir.path(), &["commit", "-q", "-m", "b"]);
        let second = git(dir.path(), &["rev-parse", "HEAD"]);
        let why = checkout_at(VAR, dir.path().as_os_str().to_owned(), &first, SUBTREE).unwrap_err();
        let fix = format!("git -C \"{}\" checkout {first}", dir.path().display());
        for wanted in [VAR, &first, &second, &fix] {
            assert!(why.contains(wanted), "{wanted} is not in: {why}");
        }
    }

    fn refusal(path: &Path, commit: &str) -> String {
        checkout_at(VAR, path.as_os_str().to_owned(), commit, SUBTREE).unwrap_err()
    }

    #[test]
    fn a_variable_nothing_sets_names_no_checkout() {
        assert_eq!(
            pilot_checkout("LVPM_TEST_PILOT_NOTHING_SETS_THIS", "abc", SUBTREE),
            Err("LVPM_TEST_PILOT_NOTHING_SETS_THIS names no checkout".to_string())
        );
    }

    #[test]
    fn a_missing_path_is_refused() {
        let (repo, head) = repo_with_a_commit();
        let missing = repo.path().join("nowhere");
        let why = refusal(&missing, &head);
        for wanted in [VAR, &missing.display().to_string(), "does not exist"] {
            assert!(why.contains(wanted), "{wanted} is not in: {why}");
        }
    }

    #[test]
    fn a_folder_that_is_not_a_repository_is_refused() {
        let (repo, head) = repo_with_a_commit();
        let inside_a_repository = repo.path().join("sub");
        std::fs::create_dir(&inside_a_repository).unwrap();
        let outside = tempfile::tempdir().unwrap();
        for path in [outside.path(), inside_a_repository.as_path()] {
            let why = refusal(path, &head);
            for wanted in [VAR, &path.display().to_string(), "not a git repository"] {
                assert!(why.contains(wanted), "{wanted} is not in: {why}");
            }
        }
    }

    #[test]
    fn a_modified_file_is_refused() {
        let (repo, head) = repo_with_a_commit();
        std::fs::write(repo.path().join(SUBTREE).join("a.txt"), "changed").unwrap();
        let why = refusal(repo.path(), &head);
        for wanted in [
            VAR,
            &repo.path().display().to_string(),
            "uncommitted changes",
        ] {
            assert!(why.contains(wanted), "{wanted} is not in: {why}");
        }
    }

    #[test]
    fn an_untracked_file_is_refused() {
        let (repo, head) = repo_with_a_commit();
        let nested = repo.path().join(SUBTREE).join("new");
        std::fs::create_dir(&nested).unwrap();
        std::fs::write(nested.join("b.txt"), "b").unwrap();
        let why = refusal(repo.path(), &head);
        assert!(why.contains("uncommitted changes"), "{why}");
    }

    #[test]
    fn a_git_ignored_file_is_refused() {
        let (repo, head) = repo_with_a_commit();
        std::fs::write(
            repo.path().join(".git/info/exclude"),
            "*.log
",
        )
        .unwrap();
        std::fs::write(repo.path().join(SUBTREE).join("run.log"), "l").unwrap();
        let why = refusal(repo.path(), &head);
        assert!(why.contains("uncommitted changes"), "{why}");
    }

    #[test]
    fn changes_outside_the_copied_folder_are_accepted() {
        let (repo, head) = repo_with_a_commit();
        std::fs::write(repo.path().join("outside.txt"), "changed").unwrap();
        std::fs::write(repo.path().join("noise.txt"), "n").unwrap();
        assert_eq!(
            checkout_at(VAR, repo.path().as_os_str().to_owned(), &head, SUBTREE),
            Ok(repo.path().to_path_buf())
        );
    }

    #[test]
    fn a_checkout_git_cannot_read_is_refused_not_accepted() {
        let (repo, head) = repo_with_a_commit();
        std::fs::write(repo.path().join(".git/index"), "not an index").unwrap();
        let why = refusal(repo.path(), &head);
        for wanted in [
            VAR,
            &repo.path().display().to_string(),
            "status",
            "index file",
        ] {
            assert!(why.contains(wanted), "{wanted} is not in: {why}");
        }
    }

    #[test]
    fn a_copied_tree_has_the_files_and_folders_of_the_original() {
        let from = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(from.path().join("a/b")).unwrap();
        std::fs::create_dir_all(from.path().join("a/empty")).unwrap();
        std::fs::write(from.path().join("top.txt"), "t").unwrap();
        std::fs::write(from.path().join("a/b/deep.txt"), "deep").unwrap();
        let to = tempfile::tempdir().unwrap();
        let copy = to.path().join("copy");
        copy_tree(from.path(), &copy);
        let shape = |root: &Path| -> Vec<(PathBuf, Option<u64>)> {
            let found = super::super::snapshot::scan(root, None, &[]).unwrap();
            found.into_iter().map(|(path, e)| (path, e.size)).collect()
        };
        assert_eq!(shape(&copy), shape(from.path()));
        assert!(copy.join("a/empty").is_dir(), "an empty folder is copied");
    }

    #[test]
    fn files_are_found_by_name_ignoring_case() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a/b")).unwrap();
        std::fs::write(dir.path().join("a/b/Lib.LVLIB"), b"").unwrap();
        let found = find_files_named(dir.path(), &["lib.lvlib".to_string()]);
        assert_eq!(found, vec![dir.path().join("a/b/Lib.LVLIB")]);
    }
}
