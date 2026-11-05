use std::io::Write;
use std::process::Command;

#[test]
fn install_prepares_new_files_by_default_with_an_opt_out() {
    let root = std::env::temp_dir().join(format!("lvpm-linkfix-install-{}", std::process::id()));
    let packages = root.join("packages");
    std::fs::create_dir_all(&packages).unwrap();
    let manifest = root.join("lvpm.toml");
    std::fs::write(
        &manifest,
        "[sources]\ndefaults = false\n[dependencies]\nlinkfix_fixture = \"*\"\n",
    )
    .unwrap();
    let xml = b"<Library LVVersion=\"24008000\"></Library>";
    let vi = include_bytes!("../tools/Relink Packages.vi");
    let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    archive
        .start_file("spec", zip::write::SimpleFileOptions::default())
        .unwrap();
    archive.write_all(b"[Package]\nName=linkfix_fixture\nVersion=1.0.0\n[File Group 0]\nTarget Dir=<application>\nReplace Mode=Always\nNum Files=2\nFile 0=vi.lib/New.lvlib\nFile 1=vi.lib/Fixture.vi\n").unwrap();
    archive
        .start_file(
            "File Group 0/vi.lib/New.lvlib",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
    archive.write_all(xml).unwrap();
    archive
        .start_file(
            "File Group 0/vi.lib/Fixture.vi",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
    archive.write_all(vi).unwrap();
    let packaged = archive.finish().unwrap().into_inner();
    let archive_path = packages.join("linkfix_fixture-1.0.0.vip");
    std::fs::write(&archive_path, &packaged).unwrap();

    for (name, flags) in [
        ("default", vec![]),
        ("legacy", vec!["--no-relink"]),
        ("opt-out", vec!["--no-linkfix"]),
        ("relink", vec!["--relink"]),
        ("preview", vec!["--dry-run"]),
        ("relink-preview", vec!["--relink", "--dry-run"]),
    ] {
        let prefix = root.join(name);
        let existing = prefix.join("LabVIEW/vi.lib/Existing.lvlib");
        std::fs::create_dir_all(existing.parent().unwrap()).unwrap();
        std::fs::write(&existing, xml).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_lvpm"))
            .current_dir(&root)
            .args(["install", "--manifest"])
            .arg(&manifest)
            .arg("--prefix")
            .arg(&prefix)
            .arg("--no-hooks")
            .args(flags)
            .env("LVPM_CONFIG_CACHE", root.join("cache"))
            .env("LVPM_CONFIG_SOURCES_LOCAL", &packages)
            .env("LOCALAPPDATA", root.join("user"))
            .env("XDG_CONFIG_HOME", root.join("config"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let installed = prefix.join("LabVIEW/vi.lib/New.lvlib");
        let installed_vi = prefix.join("LabVIEW/vi.lib/Fixture.vi");
        match name {
            "default" | "legacy" => {
                assert!(stdout.contains("linkfix: preparing installed files"));
                assert!(
                    std::fs::read_to_string(&installed)
                        .unwrap()
                        .contains("NI.LV.All.SourceOnly")
                );
                let prepared = std::fs::read(&installed_vi).unwrap();
                assert!(
                    prepared
                        .windows(b"NI.LV.All.SourceOnly".len())
                        .any(|bytes| bytes == b"NI.LV.All.SourceOnly"),
                    "{stdout}"
                );
            }
            "opt-out" | "relink" => {
                let reason = if name == "opt-out" {
                    "--no-linkfix"
                } else {
                    "--relink selects LabVIEW instead"
                };
                assert!(stdout.contains(&format!("linkfix: skipped ({reason})")));
                assert!(!stdout.contains("linkfix: preparing"));
                assert_eq!(std::fs::read(&installed).unwrap(), xml);
                assert_eq!(std::fs::read(&installed_vi).unwrap(), vi);
            }
            _ => {
                if name == "relink-preview" {
                    assert!(stdout.contains("linkfix: skipped (--relink selects LabVIEW instead)"));
                    assert!(!stdout.contains("linkfix: would prepare"));
                } else {
                    assert!(stdout.contains("linkfix: would prepare"));
                }
                assert!(!installed.exists());
                assert!(!installed_vi.exists());
            }
        }
        assert_eq!(std::fs::read(&existing).unwrap(), xml);
        assert_eq!(std::fs::read(&archive_path).unwrap(), packaged);
    }
    std::fs::remove_dir_all(root).unwrap();
}
