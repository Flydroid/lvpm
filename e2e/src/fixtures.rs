//! What each suite's work folder holds besides the two executables. The
//! repo's own VIs are built into this executable, so it needs no checkout
//! where it runs.

use std::io::Write;
use std::path::Path;

use anyhow::Result;

pub const RELINK_PACKAGES_VI: &[u8] = include_bytes!("../../tools/Relink Packages.vi");
pub const CALLER_VI: &[u8] = include_bytes!("../../tools/Test - Relink Packages E2E.vi");
pub const CALL_VI: &[u8] = include_bytes!("../../tools/E2E Call VI.vi");

/// The `local` suite: two packages in `pkgs/`, a caller VI, and a project
/// for the global install and one for the venv, with `pkgs/` as their only
/// source.
pub fn local(work: &Path) -> Result<()> {
    std::fs::create_dir_all(work.join("pkgs"))?;
    // In a folder of its own: LabVIEW searches a missing subVI under the
    // top-level VI's folder, and the venv's copy must not be found that way.
    std::fs::create_dir_all(work.join("caller"))?;
    std::fs::write(work.join("caller").join("caller.vi"), CALLER_VI)?;

    // Files into <vi.lib> and one <OS ...> location, plus both install hooks.
    vip(
        work,
        "lvpm_linux_test",
        r#"[Script VIs]
PreInstall="PreInstall.vi"
PostInstall="PostInstall.vi"

[File Group 0]
Target Dir="<vi.lib>/lvpm_linux_test"
Replace Mode="Always"
Num Files=3
File 0="Relink Packages.vi"
File 1="Test - Relink Packages E2E.vi"
File 2="E2E Call VI.vi"

[File Group 1]
Target Dir="<OS User Documents>/lvpm_linux_test"
Replace Mode="Always"
Num Files=1
File 0="readme.txt"

[File Group 2]
Target Dir="<resource>/Framework/Providers"
Replace Mode="Always"
Num Files=1
File 0="lvpm_linux_provider_test.vi"
"#,
        &[
            ("File Group 0/Relink Packages.vi", RELINK_PACKAGES_VI),
            ("File Group 0/Test - Relink Packages E2E.vi", CALLER_VI),
            ("File Group 0/E2E Call VI.vi", CALL_VI),
            ("File Group 1/readme.txt", b"lvpm e2e test\n"),
            ("File Group 2/lvpm_linux_provider_test.vi", CALL_VI),
            ("PreInstall.vi", CALL_VI),
            ("PostInstall.vi", CALL_VI),
        ],
    )?;

    // A caller and its subVI in two folders: what the relink walk must find.
    // The second Target Dir is spelled with backslashes, as some specs do.
    vip(
        work,
        "lvpm_linux_relink_test",
        r#"[File Group 0]
Target Dir="<vi.lib>/lvpm_linux_relink_test"
Replace Mode="Always"
Num Files=1
File 0="Test - Relink Packages E2E.vi"

[File Group 1]
Target Dir="<vi.lib>\lvpm_linux_relink_test\sub"
Replace Mode="Always"
Num Files=1
File 0="Relink Packages.vi"
"#,
        &[
            ("File Group 0/Test - Relink Packages E2E.vi", CALLER_VI),
            ("File Group 1/Relink Packages.vi", RELINK_PACKAGES_VI),
        ],
    )?;

    projects(
        work,
        "local",
        "[sources]\ndefaults = false\n\n",
        &["lvpm_linux_test"],
    )
}

/// The `public` suite: a project for the global install and one for the
/// venv, asking for `packages` from the public indexes.
pub fn public(work: &Path, packages: &[String]) -> Result<()> {
    let packages: Vec<&str> = packages.iter().map(String::as_str).collect();
    projects(work, "public", "", &packages)
}

fn projects(work: &Path, suite: &str, sources: &str, packages: &[&str]) -> Result<()> {
    let dependencies: String = packages.iter().map(|p| format!("{p} = \"*\"\n")).collect();
    for dir in ["global", "venv"] {
        std::fs::create_dir_all(work.join(dir))?;
        std::fs::write(
            work.join(dir).join("lvpm.toml"),
            format!(
                "[project]\nname = \"e2e-{suite}-{dir}\"\nlabview = \"2026\"\n\n\
                 {sources}[dependencies]\n{dependencies}"
            ),
        )?;
    }
    Ok(())
}

fn vip(work: &Path, name: &str, spec: &str, members: &[(&str, &[u8])]) -> Result<()> {
    let file = std::fs::File::create(work.join("pkgs").join(format!("{name}-1.0.0.1.vip")))?;
    let mut z = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();
    z.start_file("spec", options)?;
    write!(
        z,
        "[Package]\nName=\"{name}\"\nVersion=\"1.0.0.1\"\n\n{spec}"
    )?;
    for (path, bytes) in members {
        z.start_file(*path, options)?;
        z.write_all(bytes)?;
    }
    z.finish()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn scratch(name: &str) -> std::path::PathBuf {
        let work = std::env::temp_dir().join(format!("lvpm-e2e-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&work);
        work
    }

    #[test]
    fn local_has_the_packages_the_checks_install() {
        let work = scratch("local");
        local(&work).unwrap();
        let open = |name: &str| {
            let f = std::fs::File::open(work.join("pkgs").join(name)).unwrap();
            zip::ZipArchive::new(f).unwrap()
        };
        let mut z = open("lvpm_linux_relink_test-1.0.0.1.vip");
        let mut spec = String::new();
        z.by_name("spec")
            .unwrap()
            .read_to_string(&mut spec)
            .unwrap();
        assert!(
            spec.starts_with("[Package]\nName=\"lvpm_linux_relink_test\"\nVersion=\"1.0.0.1\"\n")
        );
        assert!(spec.contains(r#"Target Dir="<vi.lib>\lvpm_linux_relink_test\sub""#));
        assert_eq!(open("lvpm_linux_test-1.0.0.1.vip").len(), 8);
        assert!(work.join("caller").join("caller.vi").is_file());
        let toml = std::fs::read_to_string(work.join("venv").join("lvpm.toml")).unwrap();
        assert!(toml.contains("defaults = false") && toml.contains("lvpm_linux_test = \"*\""));
        std::fs::remove_dir_all(&work).unwrap();
    }

    #[test]
    fn public_asks_for_the_packages_from_the_default_sources() {
        let work = scratch("public");
        public(&work, &["a_pkg".into(), "b_pkg".into()]).unwrap();
        let toml = std::fs::read_to_string(work.join("global").join("lvpm.toml")).unwrap();
        assert!(!toml.contains("[sources]"));
        assert!(toml.ends_with("[dependencies]\na_pkg = \"*\"\nb_pkg = \"*\"\n"));
        assert!(!work.join("pkgs").exists());
        std::fs::remove_dir_all(&work).unwrap();
    }
}
