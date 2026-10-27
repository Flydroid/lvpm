//! A venv install unpacks each package into a staging folder beside the venv,
//! and moves it into its addon folder only when it is ready. LabVIEW loads
//! every folder under the venv as an addon, so a package being prepared there
//! would already be loaded, old-version VIs and all.

use super::env::OpEnv;
use super::session::InstallSession;
use super::uninstall::uninstall_one;
use crate::events::Event;
use crate::install::{self, Manifest};
use crate::venv::Venv;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// One package's staged folder and the addon folder it lands in. The only
/// mapping between the two.
#[derive(Debug, Clone)]
pub struct Staging {
    /// The staging folder, shared by the packages of one run.
    dir: PathBuf,
    /// `<venv>/<package>/1`.
    final_root: PathBuf,
    /// `<staging>/<package>/1`.
    stage_root: PathBuf,
}

impl Staging {
    pub(crate) fn for_package(venv: &Venv, package: &str) -> Staging {
        let dir = venv.staging_dir();
        Staging {
            stage_root: dir.join(package).join("1"),
            final_root: venv.dir.join(package).join("1"),
            dir,
        }
    }

    /// Where a file bound for the addon folder is staged.
    pub(crate) fn to_stage(&self, p: &Path) -> Result<PathBuf> {
        let rel = p.strip_prefix(&self.final_root).with_context(|| {
            format!(
                "{} is outside {}, so it cannot be staged",
                p.display(),
                self.final_root.display()
            )
        })?;
        Ok(self.stage_root.join(rel))
    }

    /// The staged package, as the resave walks it.
    pub fn stage_root(&self) -> &Path {
        &self.stage_root
    }

    /// The addon folder the package lands in.
    pub fn final_root(&self) -> &Path {
        &self.final_root
    }

    /// Make the staging folder, self-ignoring like the venv itself.
    pub(super) fn create(&self) -> Result<()> {
        std::fs::create_dir_all(&self.stage_root)
            .with_context(|| format!("creating {}", self.stage_root.display()))?;
        std::fs::write(self.dir.join(".gitignore"), "*\n")?;
        Ok(())
    }

    /// Remove the staging folder, as far as it goes. A leftover is removed by
    /// the next install.
    pub(super) fn discard(&self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Remove the staging folder an interrupted install left, before anything is
/// fetched. One that cannot go fails the install: what is in it would mix with
/// this run's packages.
pub(super) fn clear_leftover(env: &OpEnv, venv: &Venv) -> Result<()> {
    let dir = venv.staging_dir();
    if !dir.exists() {
        return Ok(());
    }
    std::fs::remove_dir_all(&dir).with_context(|| {
        format!(
            "cannot remove the staging folder {}. Close the venv's LabVIEW and delete it, then retry.",
            dir.display()
        )
    })?;
    env.sink.emit(Event::StagingLeftoverRemoved { dir });
    Ok(())
}

/// Remove a package folder that no install record names, so the package lands
/// in an empty one. True when there was one.
pub(super) fn clear_unrecorded(
    env: &OpEnv,
    session: &InstallSession,
    staging: &Staging,
    package: &str,
) -> Result<bool> {
    let dir = &staging.final_root;
    if !dir.exists() || install::is_installed(session.store(), package) {
        return Ok(false);
    }
    std::fs::remove_dir_all(dir).with_context(|| format!("removing {}", dir.display()))?;
    env.sink
        .emit(Event::UnrecordedFolderReplaced { dir: dir.clone() });
    Ok(true)
}

/// Move a staged package into its addon folder and record it there. An old
/// version is removed only now, so a package that failed to stage leaves it
/// installed. One that fails to move in (a file held open) is left
/// uninstalled; the next install clears its staging and installs it again.
pub(super) fn land(
    env: &OpEnv,
    session: &mut InstallSession,
    staging: &Staging,
    m: &Manifest,
) -> Result<()> {
    if install::is_installed(session.store(), &m.name) {
        // The old record names the same hook VIs the new version just
        // extracted, and its removal would take them.
        let hooks: Vec<(PathBuf, Vec<u8>)> = [
            &m.post_install_vi,
            &m.pre_install_vi,
            &m.pre_uninstall_vi,
            &m.post_uninstall_vi,
        ]
        .into_iter()
        .flatten()
        .filter_map(|p| Some((PathBuf::from(p), std::fs::read(p).ok()?)))
        .collect();
        uninstall_one(env, session, &m.name)?;
        for (p, bytes) in hooks {
            std::fs::write(&p, bytes).with_context(|| format!("writing {}", p.display()))?;
        }
    }
    let to = &staging.final_root;
    // What the old record did not name: the move needs the folder gone.
    if to.exists() {
        std::fs::remove_dir_all(to).with_context(|| format!("removing {}", to.display()))?;
    }
    if staging.stage_root.exists() {
        std::fs::create_dir_all(to.parent().expect("an addon folder has a parent"))?;
        move_in(&staging.stage_root, to).with_context(|| {
            format!(
                "moving {} to {}",
                staging.stage_root.display(),
                to.display()
            )
        })?;
    }
    install::write_manifest(session.store(), m)?;
    staging.discard();
    Ok(())
}

/// A rename that waits out a brief hold: on Windows an antivirus scan, or
/// the LabVIEW that just saved a file, can keep it open for a moment, and a
/// folder with an open file cannot move. A hold that lasts is copied past:
/// the resave's LabVIEW keeps a handle on a staged folder of a package with
/// classes until it closes (measured: the pilot's serializer, every run), and
/// a held folder's files still copy. The stage it leaves is cleared by
/// [`Staging::discard`], or by the next install.
fn move_in(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut wait = std::time::Duration::from_millis(100);
    loop {
        match std::fs::rename(from, to) {
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied && wait.as_secs() < 5 => {
                std::thread::sleep(wait);
                wait *= 2;
            }
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                return copy_in(from, to).inspect_err(|_| {
                    let _ = std::fs::remove_dir_all(to);
                });
            }
            done => return done,
        }
    }
}

fn copy_in(from: &Path, to: &Path) -> std::io::Result<()> {
    for file in super::resave::files_under(from) {
        let dest = to.join(file.strip_prefix(from).expect("found under it"));
        std::fs::create_dir_all(dest.parent().expect("a file has a folder"))?;
        std::fs::copy(&file, &dest)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::install::{InstallOptions, InstallPlan, PlannedPackage, apply};
    use super::super::session::InstallSession;
    use super::super::testing;
    use super::super::uninstall::uninstall_one;
    use crate::config::Config;
    use crate::events::{Event, EventSink, NullSink};
    use crate::index::Entry;
    use crate::install;
    use crate::ops::env::OpEnv;
    use crate::version::Version;
    use std::io::Write;
    use std::path::{Path, PathBuf};

    /// An `alpha` archive of `files` under `<application>`, with a
    /// PostInstall VI when `post_install`. `corrupt` spoils the first file's
    /// bytes, so the unpack fails after the staging folder is made.
    fn vip(
        dir: &Path,
        version: &str,
        files: &[&str],
        post_install: bool,
        corrupt: bool,
    ) -> PathBuf {
        let mut spec = format!("[Package]\nName=\"alpha\"\nVersion=\"{version}\"\n\n");
        if post_install {
            spec.push_str("[Script VIs]\nPostInstall=\"PostInstall.vi\"\n\n");
        }
        spec.push_str(&format!(
            "[Files]\nNum File Groups=\"1\"\n\n[File Group 0]\nTarget Dir=\"<application>\"\n\
             Replace Mode=\"Always\"\nNum Files={}\n",
            files.len()
        ));
        for (i, f) in files.iter().enumerate() {
            spec.push_str(&format!("File {i}=\"{f}\"\n"));
        }
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        writer.start_file("spec", stored).unwrap();
        writer.write_all(spec.as_bytes()).unwrap();
        for f in files {
            writer
                .start_file(format!("File Group 0/{f}"), stored)
                .unwrap();
            writer
                .write_all(format!("PAYLOAD {f} {version}").as_bytes())
                .unwrap();
        }
        if post_install {
            writer.start_file("PostInstall.vi", stored).unwrap();
            writer.write_all(b"hook").unwrap();
        }
        let mut bytes = writer.finish().unwrap().into_inner();
        if corrupt {
            let at = bytes.windows(7).position(|w| w == b"PAYLOAD").unwrap();
            bytes[at] = b'X';
        }
        let archive = dir.join(format!("alpha-{version}.vip"));
        std::fs::write(&archive, bytes).unwrap();
        archive
    }

    fn plan(archive: &Path, version: &str) -> InstallPlan {
        InstallPlan {
            entries: vec![PlannedPackage {
                entry: Entry {
                    name: "alpha".to_string(),
                    version: Version::parse(version),
                    url: archive.to_string_lossy().into_owned(),
                    md5: None,
                    display_name: None,
                    requires: Vec::new(),
                    lv_min: None,
                    source: String::new(),
                },
                expected_sha256: None,
            }],
        }
    }

    fn env(repo: &Path) -> OpEnv<'static> {
        env_on(&NullSink, repo)
    }

    fn env_on<'a>(sink: &'a dyn EventSink, repo: &Path) -> OpEnv<'a> {
        let config = Config {
            cache: repo.join("cache"),
            local_sources: Vec::new(),
            venv_dir: PathBuf::from(".lv-venv"),
        };
        OpEnv::new(
            sink,
            config,
            vec![testing::labview_2026()],
            repo.to_path_buf(),
        )
    }

    fn install(env: &OpEnv, archive: &Path, version: &str) -> anyhow::Result<()> {
        let mut session = InstallSession::new(env.destination()?);
        let opts = InstallOptions {
            no_relink: true,
            ..InstallOptions::default()
        };
        apply(env, &mut session, &plan(archive, version), &opts).map(drop)
    }

    /// Every file under `dir`, relative and with forward slashes, sorted.
    fn tree(dir: &Path) -> Vec<String> {
        fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
            for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(base, &p, out);
                } else {
                    let rel = p.strip_prefix(base).unwrap();
                    out.push(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        }
        let mut out = Vec::new();
        walk(dir, dir, &mut out);
        out.sort();
        out
    }

    #[test]
    fn a_venv_install_puts_the_files_only_in_the_package_folder_and_records_the_final_paths() {
        let repo = testing::scratch_dir("stage-final");
        let venv = testing::venv_in(&repo);
        let archive = vip(&repo, "2.0", &["lib/a.vi", "lib/b.ctl"], true, false);

        install(&env(&repo), &archive, "2.0").unwrap();

        let root = venv.dir.join("alpha").join("1");
        assert_eq!(tree(&root), ["lib/a.vi", "lib/b.ctl", "lvaddoninfo.json"]);
        assert!(!venv.staging_dir().exists(), "the staging folder is gone");
        let m = install::read_manifest(&venv.store(), "alpha").unwrap();
        // Without a relink folder the check below would hold for no path at all.
        assert!(!m.relink_folders.is_empty());
        let fin = root.to_string_lossy().replace('\\', "/");
        for p in m.files.iter().chain(&m.relink_folders) {
            assert!(p.starts_with(&fin), "{p} is not under {fin}");
        }
        let hook = m.post_install_vi.expect("the PostInstall VI is recorded");
        assert!(Path::new(&hook).is_file(), "{hook} is on disk");
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn a_package_of_documentation_only_lands_with_no_relink_folder() {
        let repo = testing::scratch_dir("stage-docs");
        let venv = testing::venv_in(&repo);
        let archive = vip(&repo, "2.0", &["docs/readme.txt"], false, false);

        install(&env(&repo), &archive, "2.0").unwrap();

        let root = venv.dir.join("alpha").join("1");
        assert_eq!(tree(&root), ["docs/readme.txt", "lvaddoninfo.json"]);
        let m = install::read_manifest(&venv.store(), "alpha").unwrap();
        assert!(m.relink_folders.is_empty());
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn an_upgrade_replaces_the_old_version_and_keeps_the_new_hook() {
        let repo = testing::scratch_dir("stage-upgrade");
        let venv = testing::venv_in(&repo);
        let env = env(&repo);
        install(
            &env,
            &vip(&repo, "2.0", &["lib/old.vi"], true, false),
            "2.0",
        )
        .unwrap();
        install(
            &env,
            &vip(&repo, "3.0", &["lib/new.vi"], true, false),
            "3.0",
        )
        .unwrap();

        let root = venv.dir.join("alpha").join("1");
        assert_eq!(tree(&root), ["lib/new.vi", "lvaddoninfo.json"]);
        assert_eq!(
            std::fs::read_to_string(root.join("lib/new.vi")).unwrap(),
            "PAYLOAD lib/new.vi 3.0"
        );
        let m = install::read_manifest(&venv.store(), "alpha").unwrap();
        assert_eq!(m.version, "3.0");
        assert!(Path::new(&m.post_install_vi.unwrap()).is_file());
        assert!(!venv.staging_dir().exists());
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn a_failed_unpack_during_an_upgrade_leaves_the_old_version_and_no_staging_folder() {
        let repo = testing::scratch_dir("stage-fail");
        let venv = testing::venv_in(&repo);
        let env = env(&repo);
        install(
            &env,
            &vip(&repo, "2.0", &["lib/old.vi"], false, false),
            "2.0",
        )
        .unwrap();

        let broken = vip(&repo, "3.0", &["lib/new.vi"], false, true);
        assert!(install(&env, &broken, "3.0").is_err());

        let root = venv.dir.join("alpha").join("1");
        assert_eq!(tree(&root), ["lib/old.vi", "lvaddoninfo.json"]);
        let m = install::read_manifest(&venv.store(), "alpha").unwrap();
        assert_eq!(m.version, "2.0");
        assert!(!venv.staging_dir().exists());
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn an_uninstall_removes_what_the_staged_install_recorded() {
        let repo = testing::scratch_dir("stage-uninstall");
        let venv = testing::venv_in(&repo);
        let env = env(&repo);
        install(&env, &vip(&repo, "2.0", &["lib/a.vi"], true, false), "2.0").unwrap();

        let mut session = InstallSession::new(env.destination().unwrap());
        uninstall_one(&env, &mut session, "alpha").unwrap();

        assert!(tree(&venv.dir.join("alpha")).is_empty());
        assert!(!install::is_installed(&venv.store(), "alpha"));
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn a_prefix_or_global_install_writes_in_place_and_stages_nothing() {
        let repo = testing::scratch_dir("stage-inplace");
        let venv = testing::venv_in(&repo);
        let archive = vip(&repo, "2.0", &["lib/a.vi"], false, false);
        let fake = testing::FakeInstallation::new(&repo);

        let lv = super::super::resave::fake::Fake::saving().install();
        let mut prefix = env(&repo);
        prefix.flags.prefix = Some(repo.join("dest"));
        install(&prefix, &archive, "2.0").unwrap();
        assert!(!tree(&repo.join("dest")).is_empty());

        let mut global = env(&repo);
        fake.place(&mut global);
        global.flags.global = true;
        global.flags.labview_version = Some("2099".to_string());
        install(&global, &archive, "2.0").unwrap();
        assert!(install::is_installed(&fake.roots(), "alpha"));

        assert!(!venv.staging_dir().exists());
        assert!(!venv.dir.join("alpha").exists());
        assert_eq!(lv.log().starts + lv.log().resaves.len(), 0, "no resave");
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn a_staging_folder_left_by_an_interrupted_install_is_removed() {
        let repo = testing::scratch_dir("stage-leftover");
        let venv = testing::venv_in(&repo);
        let left = venv.staging_dir().join("beta").join("1");
        std::fs::create_dir_all(&left).unwrap();
        std::fs::write(left.join("half.vi"), "half").unwrap();
        let archive = vip(&repo, "2.0", &["lib/a.vi"], false, false);

        let sink = testing::Recorded::default();
        install(&env_on(&sink, &repo), &archive, "2.0").unwrap();

        assert!(!venv.staging_dir().exists());
        assert!(!venv.dir.join("beta").exists(), "nothing of it landed");
        assert!(sink.events().contains(&Event::StagingLeftoverRemoved {
            dir: venv.staging_dir()
        }));
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[cfg(windows)]
    #[test]
    fn an_undeletable_leftover_staging_folder_fails_the_install() {
        use std::os::windows::fs::OpenOptionsExt;
        let repo = testing::scratch_dir("stage-held");
        let venv = testing::venv_in(&repo);
        std::fs::create_dir_all(venv.staging_dir()).unwrap();
        let held = venv.staging_dir().join("held.vi");
        std::fs::write(&held, "held").unwrap();
        // Shared with nobody, as a LabVIEW that has the VI open holds it.
        let open = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&held)
            .unwrap();
        let archive = vip(&repo, "2.0", &["lib/a.vi"], false, false);

        let why = install(&env(&repo), &archive, "2.0").unwrap_err();

        assert_eq!(
            why.to_string(),
            format!(
                "cannot remove the staging folder {}. Close the venv's LabVIEW and delete it, then retry.",
                venv.staging_dir().display()
            )
        );
        assert!(!venv.dir.join("alpha").exists(), "nothing was installed");
        drop(open);
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[cfg(windows)]
    #[test]
    fn a_move_waits_out_a_file_held_for_a_moment() {
        use std::os::windows::fs::OpenOptionsExt;
        let repo = testing::scratch_dir("stage-held-briefly");
        let (from, to) = (repo.join("staged"), repo.join("landed"));
        std::fs::create_dir_all(&from).unwrap();
        std::fs::write(from.join("a.vi"), "a").unwrap();
        let open = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(from.join("a.vi"))
            .unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            drop(open);
        });

        super::move_in(&from, &to).unwrap();

        release.join().unwrap();
        assert_eq!(std::fs::read(to.join("a.vi")).unwrap(), b"a");
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[cfg(windows)]
    #[test]
    fn a_folder_held_for_good_is_copied_in() {
        use std::os::windows::fs::OpenOptionsExt;
        let repo = testing::scratch_dir("stage-held-for-good");
        let (from, to) = (repo.join("staged"), repo.join("landed"));
        std::fs::create_dir_all(from.join("sub")).unwrap();
        std::fs::write(from.join("sub").join("a.vi"), "a").unwrap();
        // A folder handle that does not share delete, as LabVIEW holds one.
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0x1 | 0x2)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(from.join("sub"))
            .unwrap();

        super::move_in(&from, &to).unwrap();

        assert_eq!(std::fs::read(to.join("sub").join("a.vi")).unwrap(), b"a");
        drop(held);
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn a_folder_with_no_install_record_is_replaced() {
        let repo = testing::scratch_dir("stage-unrecorded");
        let venv = testing::venv_in(&repo);
        let root = venv.dir.join("alpha").join("1");
        std::fs::create_dir_all(root.join("lib")).unwrap();
        std::fs::write(root.join("by-hand.vi"), "mine").unwrap();
        std::fs::write(root.join("lib/a.vi"), "old").unwrap();
        let archive = vip(&repo, "2.0", &["lib/a.vi"], false, false);

        let sink = testing::Recorded::default();
        install(&env_on(&sink, &repo), &archive, "2.0").unwrap();

        assert_eq!(tree(&root), ["lib/a.vi", "lvaddoninfo.json"]);
        assert!(
            sink.events()
                .contains(&Event::UnrecordedFolderReplaced { dir: root.clone() })
        );
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn a_dry_run_leaves_a_leftover_staging_folder_and_an_unrecorded_folder_alone() {
        let repo = testing::scratch_dir("stage-dry");
        let venv = testing::venv_in(&repo);
        let left = venv.staging_dir().join("half.vi");
        let mine = venv.dir.join("alpha").join("1").join("by-hand.vi");
        for f in [&left, &mine] {
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, "x").unwrap();
        }
        let archive = vip(&repo, "2.0", &["lib/a.vi"], false, false);

        let env = env(&repo);
        let mut session = InstallSession::new(env.destination().unwrap());
        let opts = InstallOptions {
            dry_run: true,
            ..InstallOptions::default()
        };
        apply(&env, &mut session, &plan(&archive, "2.0"), &opts).unwrap();

        assert!(left.is_file() && mine.is_file());
        let _ = std::fs::remove_dir_all(&repo);
    }
}
