//! Relinking an installed package tree, over VI Server.
//!
//! Copying a `.vip`'s files is only half an install. The VIs land carrying the
//! linker tables their build machine wrote, so the paths they declare for their
//! subVIs and typedefs often do not resolve here — that is what makes LabVIEW
//! open its "searching for missing subVI" dialog. Loading a VI makes LabVIEW
//! resolve the links; only saving it persists that. See `tools/README.md` for
//! the measurements behind this.
//!
//! The load-resolve-save cycle itself lives in LabVIEW, in
//! `tools/Relink Package.vi`, whose whole interface is one string in and one
//! boolean out:
//!
//! ```text
//! Folder to relink (string)  ->  [walk, load, save]  ->  Done (bool)
//! ```
//!
//! So this module is only plumbing: find the folders a package put on disk,
//! feed them in one at a time, and wait for `Done`.

use crate::spec::Spec;
use crate::target::Roots;
use crate::viserver::{self, Connection, LvValue, VIRef};
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The control the folder goes into, and the indicator that says it finished.
const FOLDER_CONTROL: &str = "Folder to relink";
const DONE_INDICATOR: &str = "Done";
/// Read once `Done` goes true, if the VI has it. A string, because
/// [`LvValue`] carries scalars and strings only — an array of clusters, which
/// is what `Saved Items out` is, cannot be decoded without new wire work. JSON
/// in that string parses fine on this side.
const LOG_INDICATOR: &str = "report log out";

/// A folder as LabVIEW must receive it: separators native, no trailing one.
///
/// LabVIEW splits a Windows path on backslashes only — a forward slash is an
/// ordinary filename character. So `C:/vi.lib/Foo` arrives as a single
/// unparseable component, the relink walks nothing, and it reports success
/// over an empty list. Install manifests store forward slashes, which is how
/// a whole relink pass came back reporting nothing at all.
fn lv_folder(folder: &Path) -> String {
    let s = folder.to_string_lossy().replace('/', "\\");
    s.trim_end_matches('\\').to_string()
}

/// How many saved files to name before summarising the rest — the same
/// cut-off the dry run uses for a package's files.
const SHOWN_SAVES: usize = 6;

/// The VI's report, as lines fit to print under the folder they belong to.
///
/// The report is a JSON array of the absolute paths the VI saved. Printed raw
/// it is one line of escaped backslashes; here it becomes a count and the
/// paths relative to `folder`, capped like the dry run. An empty array still
/// says so — "walked 283, saved 0" and no report at all must not look alike.
/// Anything that is not such an array is passed through untouched, so a VI
/// that grows a different report keeps being heard.
pub fn summarize_log(log: &str, folder: &Path) -> Vec<String> {
    let Ok(saved) = serde_json::from_str::<Vec<String>>(log.trim()) else {
        return log.lines().filter(|l| !l.trim().is_empty()).map(str::to_string).collect();
    };
    if saved.is_empty() {
        return vec!["saved nothing".to_string()];
    }
    let base = lv_folder(folder) + "\\";
    let mut out = vec![format!("saved {} file(s)", saved.len())];
    for p in saved.iter().take(SHOWN_SAVES) {
        let rel = p.replace('/', "\\");
        let rel = rel.strip_prefix(&base).unwrap_or(&rel);
        out.push(format!("  {rel}"));
    }
    if saved.len() > SHOWN_SAVES {
        out.push(format!("  ... and {} more", saved.len() - SHOWN_SAVES));
    }
    out
}

/// Extensions worth relinking. Anything else a package ships — documentation,
/// palettes, DLLs — has no linker tables to fix.
const LV_EXTENSIONS: &[&str] =
    &["vi", "vit", "vim", "ctl", "ctt", "xctl", "llb", "lvlib", "lvclass", "lvproj"];

fn is_lv_item(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| LV_EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

const RELINK_VI_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/lv-src/relink-package.vi"
));

/// Materialize the bundled relink VI so LabVIEW can open it by path.
///
/// The VI is embedded in the executable at compile time, so the installed
/// executable does not depend on the source checkout or a companion file.
pub fn locate_vi() -> Result<PathBuf> {
    let p = std::env::temp_dir().join(format!(
        "lvpm-relink-package-{}.vi",
        env!("CARGO_PKG_VERSION")
    ));
    std::fs::write(&p, RELINK_VI_BYTES)
        .with_context(|| format!("materializing relink VI at {}", p.display()))?;
    Ok(p)
}

/// Drop any folder an already-kept folder contains, since the walk covers it.
///
/// Separators are normalised on the way through: a `Target Dir` carries its own
/// forward slashes, which survive `join` and would otherwise reach LabVIEW —
/// and the print-out — as `...\LabVIEW 2026\examples/10X Engineering`.
fn collapse(dirs: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = dirs.iter().map(|d| d.components().collect()).collect();
    // Shallowest first, so a parent is always seen before its children.
    dirs.sort_by_key(|d| d.components().count());
    let mut out: Vec<PathBuf> = Vec::new();
    for d in dirs {
        if !out.iter().any(|kept| d.starts_with(kept)) {
            out.push(d);
        }
    }
    out.sort();
    out
}

/// Collapse many packages' folder lists into one work list, keeping track of
/// which packages each kept folder covers.
///
/// [`collapse`] already stops one package from walking the same tree twice;
/// this is the same rule across the whole plan. Packages overlap constantly —
/// one installs into `vi.lib/addons/Foo` while another owns `vi.lib/addons`,
/// and the parent's walk covers the child. Measured on the 109-folder pass
/// that prompted this: 18 nested folders cost 17.7 of 61 minutes, and 1113 of
/// 6163 saved files were saved by more than one folder.
///
/// Shallowest first, so a parent is always kept before anything it covers; a
/// covered folder contributes its package name to the parent instead of a
/// walk of its own. Exact duplicates merge the same way.
pub fn collapse_work(work: &[(String, Vec<PathBuf>)]) -> Vec<(PathBuf, Vec<String>)> {
    let mut flat: Vec<(PathBuf, &str)> = Vec::new();
    for (pkg, dirs) in work {
        for d in dirs {
            flat.push((d.components().collect(), pkg));
        }
    }
    flat.sort_by_key(|(d, _)| d.components().count());
    let mut out: Vec<(PathBuf, Vec<String>)> = Vec::new();
    for (d, pkg) in flat {
        match out.iter_mut().find(|(kept, _)| d.starts_with(kept)) {
            Some((_, pkgs)) => {
                if !pkgs.iter().any(|p| p == pkg) {
                    pkgs.push(pkg.to_string());
                }
            }
            None => out.push((d, vec![pkg.to_string()])),
        }
    }
    out.sort();
    out
}

/// The folders to relink, one per file group: the package's own folder.
///
/// A group's `Target Dir` is where the package asked to be installed — for
/// anything with VIs in it that is a folder of its own, `<vi.lib>/addons/Foo`
/// or `<user.lib>/Foo`. That is the right unit of work: it covers the whole
/// package even where a subfolder holds the only VIs, and it stops at the
/// package boundary. Deriving folders from the installed file paths instead
/// would split one package across several sibling folders whenever its VIs sit
/// in subdirectories.
///
/// Groups with nothing to relink — palette `.mnu` files, documentation — are
/// dropped. So is the shared-root case: a group targeting `<vi.lib>` or
/// `<user.lib>` itself would put every other package inside the walk, so that
/// group falls back to the folders its own files actually landed in.
pub fn folders_for_spec(roots: &Roots, spec: &Spec) -> Result<Vec<PathBuf>> {
    let mut dirs: Vec<PathBuf> = Vec::new();

    for group in &spec.file_groups {
        let lv_files: Vec<&String> =
            group.files.iter().filter(|f| is_lv_item(Path::new(f.as_str()))).collect();
        if lv_files.is_empty() {
            continue;
        }
        // A venv install never wrote these, so there is nothing there to walk.
        if roots.venv().is_some()
            && Roots::classify(&group.target_dir)? != crate::target::TokenClass::LabView
        {
            continue;
        }
        let dir = roots.expand(&group.target_dir)?;
        if roots.is_shared_root(&dir) {
            for f in lv_files {
                if let Some(p) = dir.join(f.replace('\\', "/")).parent() {
                    dirs.push(p.to_path_buf());
                }
            }
        } else {
            dirs.push(dir);
        }
    }

    Ok(collapse(dirs))
}

/// The same answer, reconstructed from an install manifest's file list.
///
/// Only for manifests written before the folders were recorded — the package
/// folder itself is not recoverable from the file paths, so this is the
/// per-file approximation [`folders_for_spec`] exists to avoid.
pub fn folders_for(files: &[String]) -> Vec<PathBuf> {
    let dirs = files
        .iter()
        .map(String::as_str)
        .map(Path::new)
        .filter(|p| is_lv_item(p))
        .filter_map(|p| p.parent().map(Path::to_path_buf))
        .collect();
    collapse(dirs)
}

/// One VI Server session with `Relink Package.vi` loaded, reused across
/// folders. Loading it costs a round of link resolution of its own (it pulls in
/// JSONtext), so paying that once per install rather than once per folder is
/// worth the reference being held.
pub struct Relinker {
    /// Kept so the reference can be opened again: saving VIs invalidates it.
    vi: std::path::PathBuf,
    conn: Connection,
    vi_ref: VIRef,
    timeout: Duration,
    poll: Duration,
    /// False once `Done` turns out not to be writable from here; the VI's own
    /// clear then carries it alone. See [`Relinker::run`].
    can_reset_done: bool,
    /// A front-panel indicator to echo while the VI works, once the relink VI
    /// grows one. Read on the same poll as `Done`, so nothing about the loop
    /// changes when it appears — only its name has to be supplied.
    progress: Option<String>,
    /// The log indicator to read after each folder, and false once it turns
    /// out not to be readable.
    log: Option<String>,
}

/// What one folder's relink did, as far as this side can tell.
pub struct Outcome {
    pub took: Duration,
    /// Whatever the VI reported, verbatim. Empty when it reported nothing.
    pub log: String,
}

impl Relinker {
    /// Load the relink VI into the LabVIEW answering on `port`. Which LabVIEW
    /// that is — the target's own, or one started on a venv — is the caller's
    /// business; making it answer is `viserver::ensure_vi_server` or
    /// `launch::ensure_instance`.
    pub fn open(port: u16, vi: &Path, timeout: Duration) -> Result<Relinker> {
        // Polling only needs a round trip, but the initial load of the relink
        // VI itself can take a while, so give reads the full budget.
        let mut conn = Connection::connect("127.0.0.1", port, timeout)?;
        let vi_ref = match conn.open_vi_reference(vi) {
            Ok(r) => r,
            Err(e) => {
                conn.close();
                return Err(e).with_context(|| format!("opening {}", vi.display()));
            }
        };
        Ok(Relinker {
            conn,
            vi: vi.to_path_buf(),
            vi_ref,
            timeout,
            poll: Duration::from_millis(500),
            can_reset_done: true,
            progress: None,
            log: Some(LOG_INDICATOR.to_string()),
        })
    }

    /// Echo this indicator while a folder is being relinked. An unreadable
    /// name is reported once and then left alone, so naming an indicator the
    /// VI does not have yet cannot fail an install.
    pub fn watch(&mut self, indicator: Option<&str>) {
        self.progress = indicator.map(str::to_string);
    }

    /// Relink one folder, returning how long LabVIEW took.
    ///
    /// Asynchronous plus polling rather than a blocking `Run VI`, so a long
    /// relink reports progress instead of looking hung — and because the read
    /// timeout then covers a round trip, not the whole job.
    ///
    /// The VI clears `Done` when it starts, which is what makes one reference
    /// reusable across folders. It is cleared from here as well, before the run
    /// rather than after it: both writes agree, and doing it first closes the
    /// gap where a poll could still read the previous folder's `true`.
    /// Point the VI at one folder and set it going.
    fn start(&mut self, folder: &Path) -> Result<()> {
        self.conn.ctrl_val_set(self.vi_ref, FOLDER_CONTROL, LvValue::Str(lv_folder(folder)))?;

        if self.can_reset_done
            && let Err(e) = self.conn.ctrl_val_set(self.vi_ref, DONE_INDICATOR, LvValue::Bool(false))
        {
            eprintln!(
                "      note: cannot clear {DONE_INDICATOR} ({e}) — relying on the VI to clear it"
            );
            self.can_reset_done = false;
        }
        self.conn.run_vi_async(self.vi_ref)
    }

    /// Start the VI, working around error 1000 — "not in a state compatible
    /// with this operation".
    ///
    /// Two things provoke it, and both clear on their own. The VI relinks by
    /// saving VIs, and saving one from its own hierarchy invalidates the
    /// reference we hold; and `Done` going true is the diagram finishing, not
    /// the VI leaving the running state, so the next folder can arrive while
    /// LabVIEW still considers it busy. A fresh reference covers the first, a
    /// short wait the second, so each attempt does both and waits longer.
    fn start_with_retries(&mut self, folder: &Path) -> Result<()> {
        const WAITS_MS: [u64; 3] = [500, 2_000, 5_000];
        let mut last = match self.start(folder) {
            Ok(()) => return Ok(()),
            Err(e) if viserver::error_code(&e) == Some(1000) => e,
            Err(e) => return Err(e),
        };
        for (attempt, wait) in WAITS_MS.iter().enumerate() {
            std::thread::sleep(Duration::from_millis(*wait));
            if let Err(e) = self.reopen() {
                return Err(e.context("reopening the relink VI after error 1000"));
            }
            match self.start(folder) {
                Ok(()) => return Ok(()),
                Err(e) if viserver::error_code(&e) == Some(1000) => last = e,
                Err(e) => return Err(e),
            }
            if attempt + 1 == WAITS_MS.len() {
                break;
            }
        }
        Err(last).with_context(|| {
            format!(
                "the relink VI stayed busy across {} attempts over {}ms",
                WAITS_MS.len() + 1,
                WAITS_MS.iter().sum::<u64>()
            )
        })
    }

    /// Trade the reference in for a new one. The old one is released
    /// best-effort: it may be exactly what LabVIEW has already invalidated.
    fn reopen(&mut self) -> Result<()> {
        let _ = self.conn.release(self.vi_ref);
        self.vi_ref = self.conn.open_vi_reference(&self.vi)?;
        Ok(())
    }

    pub fn run(&mut self, folder: &Path) -> Result<Outcome> {
        let started = Instant::now();
        self.start_with_retries(folder)?;
        let mut last = String::new();
        loop {
            std::thread::sleep(self.poll);

            if let Some(name) = self.progress.clone() {
                match self.conn.ctrl_val_get(self.vi_ref, &name) {
                    Ok(v) => {
                        let shown = v.to_string();
                        if shown != last {
                            println!("        {:>6.0}s  {shown}", started.elapsed().as_secs_f64());
                            last = shown;
                        }
                    }
                    Err(e) => {
                        eprintln!("      note: cannot read {name} ({e}) — not reporting progress");
                        self.progress = None;
                    }
                }
            }

            match self.conn.ctrl_val_get(self.vi_ref, DONE_INDICATOR)? {
                LvValue::Bool(true) => {
                    return Ok(Outcome { took: started.elapsed(), log: self.read_log() });
                }
                LvValue::Bool(false) => {}
                other => bail!("{DONE_INDICATOR} read back as {other}, not a boolean"),
            }
            if started.elapsed() >= self.timeout {
                bail!(
                    "relink of {} still running after {}s ({DONE_INDICATOR} never went true) — \
                     LabVIEW may be showing a dialog",
                    folder.display(),
                    self.timeout.as_secs()
                );
            }
        }
    }

    /// Read the VI's own account of what it just did.
    ///
    /// Best-effort by design: a relink that worked must not be reported as
    /// failed because the VI has no log indicator, or has not grown one yet.
    /// The name is dropped after the first failure so 44 packages do not
    /// produce 44 identical notes.
    fn read_log(&mut self) -> String {
        let Some(name) = self.log.clone() else { return String::new() };
        match self.conn.ctrl_val_get(self.vi_ref, &name) {
            Ok(LvValue::Str(s)) => s,
            Ok(other) => other.to_string(),
            Err(_) => {
                self.log = None;
                String::new()
            }
        }
    }

    pub fn close(mut self) {
        let _ = self.conn.release(self.vi_ref);
        self.conn.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The report as LabVIEW writes it: a JSON array of absolute paths, which
    /// becomes a count and folder-relative names, capped past six.
    #[test]
    fn summarize_log_counts_and_relativises() {
        let folder = Path::new(r"C:\Program Files\National Instruments\LabVIEW 2026\examples\DQMH");
        let log = r#"["C:\\Program Files\\National Instruments\\LabVIEW 2026\\examples\\DQMH\\Libraries\\A\\A.lvlib", "C:\\Program Files\\National Instruments\\LabVIEW 2026\\examples\\DQMH\\B.lvlib"]"#;
        assert_eq!(
            summarize_log(log, folder),
            vec!["saved 2 file(s)", r"  Libraries\A\A.lvlib", r"  B.lvlib"]
        );

        let many: Vec<String> = (0..9).map(|i| format!(r"C:\x\{i}.vi")).collect();
        let lines = summarize_log(&serde_json::to_string(&many).unwrap(), Path::new(r"C:\x"));
        assert_eq!(lines.len(), 1 + SHOWN_SAVES + 1);
        assert_eq!(lines[0], "saved 9 file(s)");
        assert_eq!(lines[1], "  0.vi");
        assert_eq!(lines.last().unwrap(), "  ... and 3 more");

        assert_eq!(summarize_log("[]", folder), vec!["saved nothing"]);
        // Not the array we know: pass the text through, blank lines dropped.
        assert_eq!(summarize_log("walked 3\n\nsaved 0\n", folder), vec!["walked 3", "saved 0"]);
        assert!(summarize_log("", folder).is_empty());
    }

    /// The cross-package version of `collapse`: a folder covered by another
    /// package's folder joins that folder's run instead of getting its own,
    /// and exact duplicates merge. Order and per-folder attribution both
    /// matter — a failure has to be pinned on every package it walked for.
    #[test]
    fn collapse_work_merges_overlapping_packages() {
        let work = vec![
            ("caraya".to_string(), vec![PathBuf::from(r"C:\lv\vi.lib\addons\Caraya")]),
            ("h5".to_string(), vec![PathBuf::from(r"C:\lv\vi.lib\addons")]),
            ("caraya_cli".to_string(), vec![PathBuf::from(r"C:\lv\vi.lib\addons\Caraya")]),
            ("dqmh".to_string(), vec![PathBuf::from(r"C:\lv\project\DQMH")]),
        ];
        let plan = collapse_work(&work);
        assert_eq!(plan.len(), 2);
        assert_eq!(plan[0].0, PathBuf::from(r"C:\lv\project\DQMH"));
        assert_eq!(plan[0].1, ["dqmh"]);
        assert_eq!(plan[1].0, PathBuf::from(r"C:\lv\vi.lib\addons"));
        assert_eq!(plan[1].1, ["h5", "caraya", "caraya_cli"]);
    }

    /// Install manifests store forward slashes; LabVIEW needs backslashes, or
    /// it takes the whole path for one filename and relinks nothing while
    /// reporting success. A whole 101-folder pass came back empty this way.
    #[test]
    fn folders_reach_labview_with_native_separators() {
        let m = "C:/Program Files/National Instruments/LabVIEW 2026/vi.lib/Delacor/Libraries";
        assert_eq!(
            lv_folder(Path::new(m)),
            r"C:\Program Files\National Instruments\LabVIEW 2026\vi.lib\Delacor\Libraries"
        );
        // Already native, mixed, and trailing separators all land the same way.
        assert_eq!(lv_folder(Path::new(r"C:\vi.lib\Foo")), r"C:\vi.lib\Foo");
        assert_eq!(lv_folder(Path::new(r"C:\vi.lib/Foo\")), r"C:\vi.lib\Foo");
        assert_eq!(lv_folder(Path::new("C:/vi.lib/Foo/")), r"C:\vi.lib\Foo");
    }

    fn f(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|s| s.to_string()).collect()
    }

    fn group(index: usize, target_dir: &str, files: &[&str]) -> crate::spec::FileGroup {
        crate::spec::FileGroup {
            index,
            target_dir: target_dir.to_string(),
            replace_mode: "Always".to_string(),
            files: f(files),
        }
    }

    fn spec(groups: Vec<crate::spec::FileGroup>) -> Spec {
        Spec {
            name: "acme_lib_thing".into(),
            version: "1.0.0.1".into(),
            display_name: None,
            file_groups: groups,
            script_vis: Vec::new(),
            requires: None,
            lv_gate: None,
        }
    }

    /// The whole point: one folder per package, even when the VIs sit in
    /// subdirectories and nothing is at the package root.
    #[test]
    fn relinks_the_package_folder_the_spec_asked_for() {
        let roots = Roots::scratch(Path::new("/box"));
        let got = folders_for_spec(
            &roots,
            &spec(vec![
                group(1, "<vi.lib>/addons/Thing", &["sub/Deep.vi", "sub/other/More.vi"]),
                // Palettes have nothing to relink.
                group(2, "<menus>/Categories/Thing", &["thing.mnu", "dir.mnu"]),
            ]),
        )
        .unwrap();
        assert_eq!(got, vec![PathBuf::from("/box/LabVIEW/vi.lib/addons/Thing")]);
    }

    /// A group aimed at a shared root must not drag every other package into
    /// the walk, so it falls back to where its own files went.
    #[test]
    fn a_shared_root_target_falls_back_to_the_files_own_folders() {
        let roots = Roots::scratch(Path::new("/box"));
        let got = folders_for_spec(
            &roots,
            &spec(vec![group(1, "<user.lib>", &["Thing/A.vi", "Thing/nested/B.vi"])]),
        )
        .unwrap();
        assert_eq!(got, vec![PathBuf::from("/box/LabVIEW/user.lib/Thing")]);
    }

    #[test]
    fn keeps_one_folder_per_tree_and_drops_children() {
        let got = folders_for(&f(&[
            "C:/LV/vi.lib/addons/foo/Foo.lvlib",
            "C:/LV/vi.lib/addons/foo/Open.vi",
            "C:/LV/vi.lib/addons/foo/sub/Deep.vi",
            "C:/LV/user.lib/bar/Bar.vi",
        ]));
        assert_eq!(
            got,
            vec![PathBuf::from("C:/LV/user.lib/bar"), PathBuf::from("C:/LV/vi.lib/addons/foo")]
        );
    }

    #[test]
    fn siblings_stay_separate_rather_than_lifting_to_a_shared_parent() {
        // Nothing lives directly in addons/, so neither package's folder may be
        // replaced by addons/ itself.
        let got = folders_for(&f(&[
            "C:/LV/vi.lib/addons/foo/Foo.vi",
            "C:/LV/vi.lib/addons/baz/Baz.vi",
        ]));
        assert_eq!(
            got,
            vec![
                PathBuf::from("C:/LV/vi.lib/addons/baz"),
                PathBuf::from("C:/LV/vi.lib/addons/foo")
            ]
        );
    }

    #[test]
    fn ignores_files_with_no_linker_tables() {
        assert!(folders_for(&f(&[
            "C:/LV/menus/Categories/foo.mnu",
            "C:/LV/vi.lib/addons/foo/readme.html",
            "C:/LV/resource/foo.dll",
        ]))
        .is_empty());
    }
}
