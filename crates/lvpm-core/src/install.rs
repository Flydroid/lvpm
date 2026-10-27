//! Unpacking a `.vip` into a target, and taking it back out again.
//!
//! Install is a file copy: expand the `Target Dir` token against the target's
//! roots, join the file's relative path, write it. No LabVIEW process is
//! involved. An install manifest records absolute paths so uninstall can
//! remove exactly what was added and nothing else.

use crate::spec::Spec;
use crate::target::{LvTarget, Roots, TokenClass};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    pub display_name: Option<String>,
    pub installed_at: u64,
    /// Where it went: a scratch prefix or a LabVIEW install dir.
    pub target: String,
    /// Absolute paths, forward-slashed.
    pub files: Vec<String>,
    /// Hooks the package declares that were not run: every one in a venv
    /// (hooks act on a LabVIEW installation, and a venv is not one), and
    /// globally only a PostInstall that failed. Uninstall hooks are not
    /// skipped, they are pending — `lvpm uninstall` runs them from the
    /// extracted copies below. Manifests older than that extraction list them
    /// here, which is how uninstall knows it has nothing to run.
    pub skipped_hooks: Vec<String>,
    /// Whether the relink pass has run over these files since they were
    /// copied. Defaults to false so manifests written before relinking existed
    /// read as "not relinked", which is what they are.
    #[serde(default)]
    pub relinked: bool,
    /// The package's own folders, as the spec asked for them — what the relink
    /// pass walks. Recorded here because the file list alone cannot say where
    /// the package boundary was.
    #[serde(default)]
    pub relink_folders: Vec<String>,
    /// Where the package's `PostInstall.vi` was extracted to, when it ships
    /// one. The archive carries hook VIs at its root under the hook's own
    /// name — the path in `[Script VIs]` is only where the VI lived on the
    /// build machine. Recorded so the run can happen after the relink pass,
    /// and be retried.
    #[serde(default)]
    pub post_install_vi: Option<String>,
    /// Where `PreInstall.vi` was extracted to, when the package ships one. It
    /// has already run by the time this manifest exists — kept for the record
    /// and so uninstall can clean it up.
    #[serde(default)]
    pub pre_install_vi: Option<String>,
    /// Where `PreUninstall.vi` / `PostUninstall.vi` were extracted to, when
    /// the package ships them. Extracted at install time — the archive is
    /// long gone by the time uninstall needs them.
    #[serde(default)]
    pub pre_uninstall_vi: Option<String>,
    #[serde(default)]
    pub post_uninstall_vi: Option<String>,
    /// File groups a venv install left out because they aim at the machine
    /// rather than the LabVIEW tree, as `Target Dir (N files)`.
    #[serde(default)]
    pub skipped_groups: Vec<String>,
    /// The SHA-256 of the archive these files came from, as lowercase hex.
    /// Manifests written before it was recorded have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_sha256: Option<String>,
}

/// What an install *would* do. Produced first so `--dry-run` and the real run
/// share one code path.
#[derive(Debug)]
pub struct Plan {
    pub writes: Vec<PlannedWrite>,
    /// Files the package ships that were already on disk and left alone
    /// (`Replace Mode = If Newer`). Still part of its footprint: they go in
    /// the manifest so uninstall takes them with everything else.
    pub kept: Vec<PathBuf>,
    pub missing_from_archive: Vec<String>,
    /// Groups not written because a venv holds LabVIEW-tree content only:
    /// the `Target Dir` and how many files it named.
    pub skipped_groups: Vec<(String, usize)>,
}

#[derive(Debug)]
pub struct PlannedWrite {
    pub zip_index: usize,
    pub dest: PathBuf,
    pub overwrites: bool,
}

/// Reject absolute paths and `..` traversal before we join anything.
fn safe_relative(rel: &str) -> Result<PathBuf> {
    let rel = rel.replace('\\', "/");
    if rel.starts_with('/') || rel.contains(':') {
        bail!("refusing absolute path in package: {rel:?}");
    }
    let mut out = PathBuf::new();
    for part in rel.split('/') {
        match part {
            "" | "." => continue,
            ".." => bail!("refusing path traversal in package: {rel:?}"),
            p => out.push(p),
        }
    }
    Ok(out)
}

pub(crate) fn manifest_path(roots: &Roots, name: &str) -> PathBuf {
    roots.store_dir().join(format!("{name}.json"))
}

pub fn is_installed(roots: &Roots, name: &str) -> bool {
    manifest_path(roots, name).exists()
}

pub fn read_manifest(roots: &Roots, name: &str) -> Result<Manifest> {
    let p = manifest_path(roots, name);
    let text = std::fs::read_to_string(&p)
        .with_context(|| format!("{name} is not installed in this target ({})", p.display()))?;
    Ok(serde_json::from_str(&text)?)
}

pub fn list_installed(roots: &Roots) -> Result<Vec<Manifest>> {
    let dir = roots.store_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for e in std::fs::read_dir(&dir)? {
        let p = e?.path();
        if p.extension().is_some_and(|x| x == "json")
            && let Ok(text) = std::fs::read_to_string(&p)
            && let Ok(m) = serde_json::from_str::<Manifest>(&text)
        {
            out.push(m);
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

type Archive = zip::ZipArchive<std::io::Cursor<Vec<u8>>>;

pub fn open_archive(vip_bytes: Vec<u8>) -> Result<Archive> {
    Ok(zip::ZipArchive::new(std::io::Cursor::new(vip_bytes))?)
}

/// The most of a `spec` member that is read: real ones are a few KiB, and a
/// feed's or a local folder's archive must not inflate one until memory runs
/// out.
const MAX_SPEC_BYTES: u64 = 1024 * 1024;

/// The archive's `spec` member, read and parsed.
pub fn read_spec(zip: &mut Archive) -> Result<Spec> {
    // Almost always literally "spec" at the archive root.
    let idx = (0..zip.len()).find(|i| {
        zip.by_index(*i)
            .map(|f| f.name().eq_ignore_ascii_case("spec"))
            .unwrap_or(false)
    });
    let Some(idx) = idx else {
        bail!("no `spec` member in .vip archive");
    };
    let mut buf = Vec::new();
    zip.by_index(idx)?
        .take(MAX_SPEC_BYTES + 1)
        .read_to_end(&mut buf)?;
    if buf.len() as u64 > MAX_SPEC_BYTES {
        bail!("the `spec` member is larger than {MAX_SPEC_BYTES} bytes");
    }
    // spec files are latin-1-ish; lossy is fine for the fields we read.
    crate::spec::parse(&String::from_utf8_lossy(&buf))
}

/// Work out every file this package would write, without touching disk.
pub fn plan(roots: &Roots, spec: &Spec, zip: &mut Archive) -> Result<Plan> {
    // Zip entry names are case-sensitive; index once, tolerate case drift
    // between `spec` and the archive.
    let mut by_lower = std::collections::HashMap::new();
    for i in 0..zip.len() {
        let name = zip.by_index(i)?.name().to_string();
        by_lower.insert(name.to_lowercase().replace('\\', "/"), i);
    }

    let mut writes = Vec::new();
    let mut kept = Vec::new();
    let mut missing = Vec::new();
    let mut skipped = Vec::new();

    for group in &spec.file_groups {
        if group.files.is_empty() {
            continue;
        }
        // A venv is an addon: LabVIEW-tree content only. What a package also
        // puts on the machine — a driver DLL into System32, say — is no part
        // of that, and a project's install must not scatter files across the
        // machine. Left out, and said so.
        if roots.venv().is_some() && Roots::classify(&group.target_dir)? != TokenClass::LabView {
            skipped.push((group.target_dir.clone(), group.files.len()));
            continue;
        }
        let dest_root = roots.expand(&group.target_dir)?;
        let if_newer = group.replace_mode.eq_ignore_ascii_case("If Newer");

        for rel in &group.files {
            let rel_path = safe_relative(rel)?;
            let entry_name = format!("File Group {}/{}", group.index, rel.replace('\\', "/"));

            let Some(&zip_index) = by_lower.get(&entry_name.to_lowercase()) else {
                missing.push(entry_name);
                continue;
            };

            let dest = dest_root.join(&rel_path);
            roots.check_contained(&dest)?;

            let exists = dest.exists();
            if exists && if_newer {
                // PoC simplification: `If Newer` only writes when absent.
                // Doing it properly means comparing the archive timestamp
                // against the file on disk.
                kept.push(dest);
                continue;
            }
            writes.push(PlannedWrite {
                zip_index,
                dest,
                overwrites: exists,
            });
        }
    }

    Ok(Plan {
        writes,
        kept,
        missing_from_archive: missing,
        skipped_groups: skipped,
    })
}

/// The `lvaddoninfo.json` LabVIEW reads to mount an addon (fields as NI's own
/// addons write them). The floor is the bound target itself, not what the
/// package claimed: the relink pass saves the payload in that version's
/// format, so nothing older could load it anyway.
pub(crate) fn addon_info(name: &str, target: &LvTarget) -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "AddonName": name,
        "ApiVersion": "v1",
        "MinimumSupportedLVVersion": format!("{}.0", target.version.trunc() as u32),
        "SupportedBitness": target.bitness.to_string(),
    }))
    .expect("a fixed shape of strings serialises")
}

/// Execute a plan and record the manifest, with the SHA-256 of the archive
/// the files were read from.
pub fn apply(
    roots: &Roots,
    spec: &Spec,
    zip: &mut Archive,
    plan: &Plan,
    pre_install_vi: Option<&Path>,
    archive_sha256: &str,
) -> Result<Manifest> {
    let mut written = Vec::new();

    // A declared PostInstall hook ships as `PostInstall.vi` at the archive
    // root. Extract it next to the manifests so it survives until the relink
    // pass has made it runnable.
    let mut hook = |name: &str, member: &str| -> Result<Option<PathBuf>> {
        match spec
            .script_vis
            .iter()
            .any(|(h, v)| h == name && !v.is_empty())
        {
            true => extract_hook(roots, &spec.name, zip, member),
            false => Ok(None),
        }
    };
    let post_install_vi = hook("PostInstall", "PostInstall.vi")?;
    let pre_uninstall_vi = hook("PreUninstall", "PreUninstall.vi")?;
    let post_uninstall_vi = hook("PostUninstall", "PostUninstall.vi")?;

    for w in &plan.writes {
        if let Some(parent) = w.dest.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let mut src = zip.by_index(w.zip_index)?;
        let mut buf = Vec::with_capacity(src.size() as usize);
        src.read_to_end(&mut buf)?;
        std::fs::write(&w.dest, &buf).with_context(|| format!("writing {}", w.dest.display()))?;
        written.push(w.dest.to_string_lossy().replace('\\', "/"));
    }
    // The footprint is what the package ships, not what this run happened to
    // copy — a file left alone today is still gone when the package goes.
    for k in &plan.kept {
        written.push(k.to_string_lossy().replace('\\', "/"));
    }

    // An addon is what LVAddons mounts: `lvaddoninfo.json` beside the mirrored
    // tree names it and says which LabVIEW may load it. Only when something
    // landed — an addon holding nothing but its info file is noise, and a
    // package whose every group aimed at the machine leaves no addon at all.
    if roots.venv().is_some()
        && let Some(t) = &roots.target
        && !written.is_empty()
    {
        let info = roots.application.join("lvaddoninfo.json");
        std::fs::create_dir_all(&roots.application)?;
        std::fs::write(&info, addon_info(&spec.name, t))
            .with_context(|| format!("writing {}", info.display()))?;
        written.push(info.to_string_lossy().replace('\\', "/"));
    }

    let manifest = Manifest {
        name: spec.name.clone(),
        version: spec.version.clone(),
        display_name: spec.display_name.clone(),
        installed_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        target: match &roots.target {
            Some(t) => t.label(),
            None => format!("scratch {}", roots.application.display()),
        },
        files: written,
        skipped_hooks: match roots.venv() {
            Some(_) => spec
                .script_vis
                .iter()
                .map(|(h, v)| format!("{h}={v}"))
                .collect(),
            // PreInstall has run by now, PostInstall runs after the relink
            // pass and is recorded here only if it fails, and the uninstall
            // hooks wait for `lvpm uninstall` in the copies extracted above.
            None => Vec::new(),
        },
        relinked: false,
        post_install_vi: post_install_vi.map(|p| p.to_string_lossy().replace('\\', "/")),
        pre_install_vi: pre_install_vi.map(|p| p.to_string_lossy().replace('\\', "/")),
        pre_uninstall_vi: pre_uninstall_vi.map(|p| p.to_string_lossy().replace('\\', "/")),
        post_uninstall_vi: post_uninstall_vi.map(|p| p.to_string_lossy().replace('\\', "/")),
        relink_folders: crate::relink::folders_for_spec(roots, spec)?
            .iter()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect(),
        skipped_groups: plan
            .skipped_groups
            .iter()
            .map(|(dir, n)| format!("{dir} ({n} files)"))
            .collect(),
        archive_sha256: Some(archive_sha256.to_string()),
    };

    write_manifest(roots, &manifest)?;
    Ok(manifest)
}

/// Pull one hook VI out of the archive root into the target's `hooks` store.
/// A declared hook missing from the archive is a warning-level oddity, not an
/// error — the files themselves installed fine.
pub fn extract_hook(
    roots: &Roots,
    package: &str,
    zip: &mut Archive,
    member: &str,
) -> Result<Option<PathBuf>> {
    let Ok(mut src) = zip.by_name(member) else {
        return Ok(None);
    };
    let dir = roots.hooks_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let dest = dir.join(format!("{package}-{member}"));
    let mut buf = Vec::with_capacity(src.size() as usize);
    src.read_to_end(&mut buf)?;
    std::fs::write(&dest, &buf).with_context(|| format!("writing {}", dest.display()))?;
    Ok(Some(dest))
}

fn write_manifest(roots: &Roots, m: &Manifest) -> Result<()> {
    let mp = manifest_path(roots, &m.name);
    std::fs::create_dir_all(mp.parent().unwrap())?;
    std::fs::write(&mp, serde_json::to_string_pretty(m)?)?;
    Ok(())
}

/// Record that the relink pass has run over this package.
///
/// Separate from `apply` because the two happen in different phases: every
/// package is copied before any is relinked, so the manifest is written once
/// with `relinked: false` and amended if and when the relink succeeds.
pub fn mark_relinked(roots: &Roots, name: &str) -> Result<()> {
    let mut m = read_manifest(roots, name)?;
    if m.relinked {
        return Ok(());
    }
    m.relinked = true;
    write_manifest(roots, &m)
}

/// Record a hook this install declared and did not run, so `lvpm list` says
/// so. `hook` is the `Name=path` form the manifest already uses.
pub fn mark_hook_skipped(roots: &Roots, name: &str, hook: &str) -> Result<()> {
    let mut m = read_manifest(roots, name)?;
    if m.skipped_hooks.iter().any(|h| h == hook) {
        return Ok(());
    }
    m.skipped_hooks.push(hook.to_string());
    write_manifest(roots, &m)
}

/// The install record is a file on disk, and uninstall deletes and runs what
/// it names: refuse a record that names a file outside the places this
/// destination installs to, or a hook VI outside its hooks directory.
pub fn check_record(roots: &Roots, manifest: &Manifest) -> Result<()> {
    let hooks = roots.hooks_dir();
    let hook_vis = [
        &manifest.post_install_vi,
        &manifest.pre_install_vi,
        &manifest.pre_uninstall_vi,
        &manifest.post_uninstall_vi,
    ]
    .into_iter()
    .flatten();
    let outside: Vec<&String> = manifest
        .files
        .iter()
        .filter(|f| !roots.is_inside(Path::new(f)))
        .chain(hook_vis.filter(|h| !crate::target::within(&hooks, Path::new(h))))
        .collect();
    if let Some(first) = outside.first() {
        bail!(
            "refusing {}: its install record names {} path(s) outside this \
             destination, the first {first:?}; nothing was removed or run",
            manifest.name,
            outside.len()
        );
    }
    Ok(())
}

/// Remove every file the manifest recorded, then prune directories this
/// leaves empty. Directories that still hold anything are left alone.
pub fn uninstall(roots: &Roots, name: &str) -> Result<(usize, Manifest)> {
    let manifest = read_manifest(roots, name)?;
    check_record(roots, &manifest)?;
    let mut removed = 0;
    let mut dirs: Vec<PathBuf> = Vec::new();

    let stop_at = roots.application.clone();

    for f in &manifest.files {
        let p = PathBuf::from(f);
        if p.exists() {
            std::fs::remove_file(&p).with_context(|| format!("removing {}", p.display()))?;
            removed += 1;
        }
        let mut d = p.parent().map(Path::to_path_buf);
        while let Some(dir) = d {
            // Never prune the LabVIEW install dir itself or anything above it.
            if dir == stop_at || dir.parent().is_none() {
                break;
            }
            dirs.push(dir.clone());
            d = dir.parent().map(Path::to_path_buf);
        }
    }

    // Deepest first, so parents get a chance once children are gone.
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    dirs.dedup();
    for d in dirs {
        if d.exists() && std::fs::read_dir(&d)?.next().is_none() {
            let _ = std::fs::remove_dir(&d);
        }
    }

    // The extracted hook VIs go with the package they belonged to.
    for hook in [
        &manifest.post_install_vi,
        &manifest.pre_install_vi,
        &manifest.pre_uninstall_vi,
        &manifest.post_uninstall_vi,
    ]
    .into_iter()
    .flatten()
    {
        let _ = std::fs::remove_file(hook);
    }
    std::fs::remove_file(manifest_path(roots, name))?;
    Ok((removed, manifest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_traversal_and_absolute_paths() {
        assert!(safe_relative("../../etc/passwd").is_err());
        assert!(safe_relative("/etc/passwd").is_err());
        assert!(safe_relative(r"C:\windows\system32\evil.dll").is_err());
        assert!(safe_relative("vi.lib/addons/ok.llb").is_ok());
    }

    /// An archive whose only member is `spec`, holding `text`, deflated.
    fn archive_with_spec(text: &[u8]) -> Archive {
        use std::io::Write;
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let deflated = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("spec", deflated).unwrap();
        writer.write_all(text).unwrap();
        open_archive(writer.finish().unwrap().into_inner()).unwrap()
    }

    const SPEC: &[u8] = b"[Package]\nName=alpha\nVersion=1.0\n";

    /// [`SPEC`] padded with spaces to `len` bytes: a few KiB deflated.
    fn padded_spec(len: u64) -> Vec<u8> {
        let mut text = SPEC.to_vec();
        text.resize(len as usize, b' ');
        text
    }

    #[test]
    fn a_spec_member_is_read_and_parsed() {
        let read = read_spec(&mut archive_with_spec(SPEC)).unwrap();
        assert_eq!(read.name, "alpha");
    }

    #[test]
    fn a_spec_member_of_exactly_the_cap_is_read() {
        let mut zip = archive_with_spec(&padded_spec(MAX_SPEC_BYTES));
        assert_eq!(read_spec(&mut zip).unwrap().name, "alpha");
    }

    #[test]
    fn a_spec_member_over_the_cap_is_refused() {
        let mut zip = archive_with_spec(&padded_spec(MAX_SPEC_BYTES + 1));
        let why = read_spec(&mut zip).unwrap_err().to_string();
        assert!(why.contains("larger than"), "{why}");
    }

    #[test]
    fn an_archive_with_no_spec_member_is_refused() {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        writer
            .start_file("other", zip::write::SimpleFileOptions::default())
            .unwrap();
        let mut zip = open_archive(writer.finish().unwrap().into_inner()).unwrap();
        let why = read_spec(&mut zip).unwrap_err().to_string();
        assert_eq!(why, "no `spec` member in .vip archive");
    }

    /// A record for `files`, with the hook VIs it is given, installed nowhere.
    fn record(files: &[&str], pre_uninstall_vi: Option<&str>) -> Manifest {
        Manifest {
            name: "a".into(),
            version: "1".into(),
            display_name: None,
            installed_at: 0,
            target: "t".into(),
            files: files.iter().map(|f| f.to_string()).collect(),
            skipped_hooks: Vec::new(),
            relinked: false,
            relink_folders: Vec::new(),
            post_install_vi: None,
            pre_install_vi: None,
            pre_uninstall_vi: pre_uninstall_vi.map(String::from),
            post_uninstall_vi: None,
            skipped_groups: Vec::new(),
            archive_sha256: None,
        }
    }

    #[test]
    fn a_record_is_refused_for_a_file_outside_the_destination_or_a_hook_outside_its_directory() {
        let roots = Roots::scratch(Path::new("/box"));
        let hooks = roots.hooks_dir().to_string_lossy().replace('\\', "/");
        let ok = record(
            &["/box/LabVIEW/a.txt"],
            Some(&format!("{hooks}/a-PreUninstall.vi")),
        );
        check_record(&roots, &ok).unwrap();

        for files in [
            &["/etc/passwd"][..],
            &["/box/../etc/passwd"],
            &["/box/LabVIEW/../../etc/passwd"],
            &["a/relative.txt"],
            &["/box/LabVIEW/a.txt", "/boxed/b.txt"],
        ] {
            let why = check_record(&roots, &record(files, None)).unwrap_err();
            assert!(
                why.to_string().contains("outside this destination"),
                "{files:?}: {why}"
            );
        }
        let hook_elsewhere = record(&["/box/LabVIEW/a.txt"], Some("/box/LabVIEW/evil.vi"));
        assert!(check_record(&roots, &hook_elsewhere).is_err());
    }

    #[test]
    fn a_manifest_from_before_the_archive_hash_reads_without_one_and_is_written_back_as_it_was() {
        let legacy = r#"{"name":"a","version":"1","display_name":null,"installed_at":0,
            "target":"t","files":[],"skipped_hooks":[]}"#;
        let manifest: Manifest = serde_json::from_str(legacy).unwrap();
        assert_eq!(manifest.archive_sha256, None);
        let written = serde_json::to_string(&manifest).unwrap();
        assert!(!written.contains("archive_sha256"), "{written}");
    }

    #[test]
    fn addon_info_has_the_fields_ni_writes_and_floors_at_the_bound_target() {
        let t = LvTarget {
            version: 26.3,
            bitness: 64,
            path: PathBuf::from(r"C:\LV"),
        };
        let v: serde_json::Value = serde_json::from_str(&addon_info("oglib_error", &t)).unwrap();
        assert_eq!(v["AddonName"], "oglib_error");
        assert_eq!(v["ApiVersion"], "v1");
        assert_eq!(v["MinimumSupportedLVVersion"], "26.0");
        assert_eq!(v["SupportedBitness"], "64");
    }
}
