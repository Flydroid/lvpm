use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "lvpm-dependency-test-{}-{name}",
            std::process::id()
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(root.join("packages")).unwrap();
        std::fs::write(root.join("lvpm.toml"), "[sources]\ndefaults = false\n").unwrap();
        Self { root }
    }

    fn package(&self, name: &str, dependencies: &str) {
        let file =
            std::fs::File::create(self.root.join("packages").join(format!("{name}-1.0.vip")))
                .unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("spec", options).unwrap();
        write!(
            zip,
            "[Package]\nName={name}\nVersion=1.0\n[Dependencies]\nRequires={dependencies}\n\
            [File Group 0]\nTarget Dir=<vi.lib>/{name}\nNum Files=1\nFile 0=file.txt\n"
        )
        .unwrap();
        zip.start_file("File Group 0/file.txt", options).unwrap();
        zip.write_all(b"fixture").unwrap();
        zip.finish().unwrap();
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_lvpm"))
            .current_dir(&self.root)
            .env("LVPM_CONFIG_CACHE", self.root.join("cache"))
            .env("LVPM_CONFIG_SOURCES_LOCAL", self.root.join("packages"))
            .arg("--prefix")
            .arg(self.root.join("prefix"))
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn record(&self, name: &str) -> PathBuf {
        self.root
            .join("prefix")
            .join(".lvpm")
            .join("installed")
            .join(format!("{name}.json"))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn text(path: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn install_records_and_uninstall_protects_shared_dependencies() {
    let f = Fixture::new("shared");
    f.package("root", "private>=1.0");
    f.package("private", "shared");
    f.package("other", "shared");
    f.package("shared", "");
    success(&f.run(&["install", "root", "--no-relink"]));
    success(&f.run(&["install", "other", "--no-relink"]));
    assert_eq!(
        text(&f.record("root"))["dependencies"],
        serde_json::json!(["private"])
    );
    assert_eq!(
        text(&f.record("private"))["dependencies"],
        serde_json::json!(["shared"])
    );

    let output = f.run(&["uninstall", "root"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--with-deps or --keep-deps"));
    assert!(f.record("root").exists());
    let output = f.run(&["uninstall", "root", "--with-deps"]);
    success(&output);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("keeping shared: still required by other")
    );
    assert!(!f.record("root").exists());
    assert!(!f.record("private").exists());
    assert!(f.record("shared").exists());
    let output = f.run(&["uninstall", "shared", "--keep-deps"]);
    success(&output);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("required by installed packages: other")
    );
}

#[test]
fn old_records_are_not_assumed_to_have_no_dependencies() {
    let f = Fixture::new("old-records");
    f.package("root", "child");
    f.package("child", "");
    f.package("old", "");
    success(&f.run(&["install", "root", "--no-relink"]));
    success(&f.run(&["install", "old", "--no-relink"]));
    let path = f.record("old");
    let mut record = text(&path);
    record.as_object_mut().unwrap().remove("dependencies");
    std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    let output = f.run(&["uninstall", "root", "--with-deps"]);
    success(&output);
    assert!(String::from_utf8_lossy(&output.stderr).contains("dependency checks are incomplete"));
    assert!(f.record("child").exists());
}

#[test]
fn corrupt_installed_dependency_record_fails_the_install_explicitly() {
    let f = Fixture::new("corrupt-record");
    f.package("root", "child");
    f.package("child", "");
    success(&f.run(&["install", "root", "--no-relink"]));
    std::fs::write(f.record("child"), b"invalid json").unwrap();
    let output = f.run(&["install", "root", "--no-relink"]);
    assert!(!output.status.success());
    assert!(f.record("root").exists());
    assert!(String::from_utf8_lossy(&output.stderr).contains("expected value"));
}
