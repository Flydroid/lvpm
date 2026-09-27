//! LabVIEW targets, and the directory roots that `Target Dir` tokens resolve to.
//!
//! A scratch prefix and a real LabVIEW installation differ only in where the
//! roots point, so both go through `Roots`. That keeps the installer honest:
//! the code path exercised against a sandbox is the same one that writes into
//! `C:\Program Files\National Instruments\LabVIEW 2026` — or, on Linux,
//! `/usr/local/natinst/LabVIEW-2026-64`.

use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct LvTarget {
    /// Internal version, e.g. 26.3.
    pub version: f64,
    pub bitness: u8,
    pub path: PathBuf,
}

impl LvTarget {
    /// Marketing year, derived from the internal version (26.x -> 2026).
    pub fn year(&self) -> u32 {
        2000 + self.version.trunc() as u32
    }

    pub fn key(&self) -> String {
        format!("LabVIEW-{}-{}bit", self.year(), self.bitness)
    }

    pub fn label(&self) -> String {
        format!("LabVIEW {} ({}-bit)  v{}", self.year(), self.bitness, self.version)
    }

    /// The executable lvpm starts. On Linux, `labview` is the install dir's
    /// symlink to whichever edition is installed (`labviewprofull`, ...).
    pub fn exe(&self) -> PathBuf {
        self.path.join(EXE)
    }

    /// The preferences file a LabVIEW started as [`LvTarget::exe`] reads:
    /// VI Server's switch and port, and what a venv's ini is copied from.
    ///
    /// On Windows that is `LabVIEW.ini` beside the executable. On Linux it is
    /// per user, `~/natinst/.config/LabVIEW-<year>/`, and named after the name
    /// LabVIEW was started as — `labview.conf` for `labview`, while the
    /// `/usr/local/bin/labview64` symlink reads `labview64.conf`. It does not
    /// exist before that user's first launch.
    pub fn ini(&self) -> PathBuf {
        #[cfg(windows)]
        {
            self.path.join("LabVIEW.ini")
        }
        #[cfg(target_os = "linux")]
        {
            env_path("HOME", "/root")
                .join("natinst/.config")
                .join(format!("LabVIEW-{}", self.year()))
                .join(format!("{EXE}.conf"))
        }
    }
}

#[cfg(windows)]
const EXE: &str = "LabVIEW.exe";
#[cfg(target_os = "linux")]
const EXE: &str = "labview";

#[cfg(windows)]
pub fn detect() -> Result<Vec<LvTarget>> {
    use winreg::RegKey;
    use winreg::enums::*;

    let mut out: Vec<LvTarget> = Vec::new();
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);

    for (flags, bitness) in [(KEY_WOW64_64KEY, 64u8), (KEY_WOW64_32KEY, 32u8)] {
        let Ok(root) =
            hklm.open_subkey_with_flags(r"SOFTWARE\National Instruments\LabVIEW", KEY_READ | flags)
        else {
            continue;
        };
        for name in root.enum_keys().flatten() {
            // Version subkeys look like "26.3"; skip "AddOns", "CurrentVersion".
            let Ok(version) = name.parse::<f64>() else { continue };
            let Ok(sub) = root.open_subkey_with_flags(&name, KEY_READ | flags) else { continue };
            let Ok(path) = sub.get_value::<String, _>("Path") else { continue };
            if path.trim().is_empty() {
                continue;
            }
            let path = PathBuf::from(path.trim_end_matches(['\\', '/']));
            if !path.join(EXE).exists() {
                continue;
            }
            // Several internal versions share one directory (26.0 and 26.3);
            // keep the highest per install path.
            match out.iter_mut().find(|t| t.path == path && t.bitness == bitness) {
                Some(existing) => existing.version = existing.version.max(version),
                None => out.push(LvTarget { version, bitness, path }),
            }
        }
    }

    out.sort_by(|a, b| b.version.partial_cmp(&a.version).unwrap_or(std::cmp::Ordering::Equal));
    Ok(out)
}

#[cfg(target_os = "linux")]
pub fn detect() -> Result<Vec<LvTarget>> {
    // LabVIEW on Linux lives under /usr/local/natinst/LabVIEW-<year>-64 unless
    // its prefix was moved at install time; either way its package links
    // /etc/natinst/labview-<year>-64 to the install's `etc`, where
    // `labview.dir` names the install. A directory without the executable is
    // what an uninstall leaves behind.
    let listed = |dir: &str| std::fs::read_dir(dir).into_iter().flatten().flatten();
    let linked = listed("/etc/natinst")
        .filter(|e| e.file_name().to_string_lossy().starts_with("labview-"))
        .filter_map(|e| std::fs::read_to_string(e.path().join("labview.dir")).ok())
        .map(|d| PathBuf::from(d.trim()));
    let mut out: Vec<LvTarget> = Vec::new();
    for p in linked.chain(listed("/usr/local/natinst").map(|e| e.path())) {
        let n = p.file_name().unwrap_or_default().to_string_lossy().to_string();
        if let Some(rest) = n.strip_prefix("LabVIEW-")
            && let Some(year) = rest.split('-').next().and_then(|y| y.parse::<u32>().ok())
            && year > 2000
            && p.join(EXE).is_file()
            && !out.iter().any(|t| t.path == p)
        {
            let bitness = if n.ends_with("-64") { 64 } else { 32 };
            let version = linux_version(&p).unwrap_or((year - 2000) as f64);
            out.push(LvTarget { version, bitness, path: p });
        }
    }
    out.sort_by(|a, b| b.version.partial_cmp(&a.version).unwrap_or(std::cmp::Ordering::Equal));
    Ok(out)
}

/// The internal version of a Linux installation, which its directory name
/// does not carry: a 2026 Q3 is 26.3, and that is what a package's
/// `Exclusive_LabVIEW_Version` gate is compared against. NI's uninstall
/// script in the install dir states it as `LV_MAJOR_VER=26` / `LV_MINOR_VER=3`.
#[cfg(target_os = "linux")]
fn linux_version(install: &Path) -> Option<f64> {
    let text = std::fs::read_to_string(install.join("readme/UNINSTALL")).ok()?;
    let var = |key: &str| {
        text.lines().find_map(|l| l.trim().strip_prefix(key)?.strip_prefix('=')?.trim().parse::<u32>().ok())
    };
    format!("{}.{}", var("LV_MAJOR_VER")?, var("LV_MINOR_VER")?).parse().ok()
}

/// Pick a target by year ("2026"), internal version ("26.3") or path.
pub fn select(targets: &[LvTarget], want: &str) -> Result<LvTarget> {
    let want = want.trim();

    if let Some(t) = targets.iter().find(|t| t.path == Path::new(want)) {
        return Ok(t.clone());
    }
    if let Ok(year) = want.parse::<u32>()
        && year > 1900
        && let Some(t) = targets.iter().find(|t| t.year() == year)
    {
        return Ok(t.clone());
    }
    if let Ok(v) = want.parse::<f64>()
        && let Some(t) = targets.iter().find(|t| (t.version - v).abs() < 1e-9)
    {
        return Ok(t.clone());
    }

    let known: Vec<String> = targets.iter().map(|t| t.year().to_string()).collect();
    bail!("no LabVIEW target matching {want:?} (detected: {})", known.join(", "));
}

/// Which kind of place a `Target Dir` token names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenClass {
    /// Inside the LabVIEW tree — `<vi.lib>`, `<menus>`, `<application>`, …
    LabView,
    /// A machine location — the `<OS …>` family.
    Os,
    /// The installer's own scratch space.
    Temp,
}

/// Where each `Target Dir` token points.
#[derive(Debug, Clone)]
pub struct Roots {
    pub application: PathBuf,
    temp: PathBuf,
    program_data: PathBuf,
    program_files: PathBuf,
    public_documents: PathBuf,
    user_documents: PathBuf,
    user_desktop: PathBuf,
    user_appdata: PathBuf,
    boot_volume: PathBuf,
    system_core: PathBuf,
    /// Set when this is a sandbox, so nothing can escape the prefix.
    scratch: Option<PathBuf>,
    /// Set when this is a project venv: the `.project` dir that LabVIEW mounts
    /// as an LVAddons location. Nothing LabVIEW-class may land outside it.
    venv: Option<PathBuf>,
    pub target: Option<LvTarget>,
}

fn env_path(key: &str, fallback: &str) -> PathBuf {
    PathBuf::from(std::env::var(key).unwrap_or_else(|_| fallback.to_string()))
}

impl Roots {
    /// Everything under one directory. Nothing outside it is ever touched.
    pub fn scratch(prefix: &Path) -> Roots {
        let p = prefix.to_path_buf();
        Roots {
            application: p.join("LabVIEW"),
            temp: p.join("temp"),
            program_data: p.join("os/ProgramData"),
            program_files: p.join("os/ProgramFiles"),
            public_documents: p.join("os/PublicDocuments"),
            user_documents: p.join("os/UserDocuments"),
            user_desktop: p.join("os/UserDesktop"),
            user_appdata: p.join("os/UserAppData"),
            boot_volume: p.join("os/BootVolume"),
            system_core: p.join("os/SystemCore"),
            scratch: Some(p),
            venv: None,
            target: None,
        }
    }

    #[cfg(windows)]
    pub fn labview(t: &LvTarget) -> Roots {
        let userprofile = env_path("USERPROFILE", "C:\\Users\\Default");
        let program_files = if t.bitness == 32 {
            env_path("ProgramFiles(x86)", "C:\\Program Files (x86)")
        } else {
            env_path("ProgramFiles", "C:\\Program Files")
        };
        Roots {
            application: t.path.clone(),
            temp: std::env::temp_dir(),
            program_data: env_path("ProgramData", "C:\\ProgramData"),
            program_files,
            public_documents: env_path("PUBLIC", "C:\\Users\\Public").join("Documents"),
            user_documents: userprofile.join("Documents"),
            user_desktop: userprofile.join("Desktop"),
            user_appdata: env_path("APPDATA", "C:\\Users\\Default\\AppData\\Roaming"),
            boot_volume: PathBuf::from(
                std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into()) + "\\",
            ),
            system_core: env_path("SystemRoot", "C:\\Windows").join("System32"),
            scratch: None,
            venv: None,
            target: Some(t.clone()),
        }
    }

    /// The machine roots are what LabVIEW's own `Get System Directory.vi`
    /// answers on Linux (2026 Q3, read over VI Server): the `<OS ...>` tokens
    /// are that VI's directory types by name.
    #[cfg(target_os = "linux")]
    pub fn labview(t: &LvTarget) -> Roots {
        let home = env_path("HOME", "/root");
        Roots {
            application: t.path.clone(),
            temp: std::env::temp_dir(),
            program_data: PathBuf::from("/usr/local"),
            program_files: PathBuf::from("/usr/local"),
            public_documents: PathBuf::from("/usr/local"),
            user_documents: home.join("Documents"),
            user_desktop: home.join("Desktop"),
            user_appdata: home,
            boot_volume: PathBuf::from("/"),
            system_core: PathBuf::from("/usr/lib"),
            scratch: None,
            venv: None,
            target: Some(t.clone()),
        }
    }

    /// One package's addon inside a project venv. `<venv>/<pkg>/1` mirrors the
    /// LabVIEW install dir — that is what LVAddons overlays — so every
    /// LabVIEW-tree token lands inside the addon. The machine roots stay real:
    /// the venv policy skips those groups rather than redirecting them.
    pub fn project(venv: &Path, target: &LvTarget, pkg: &str) -> Roots {
        let mut r = Roots::labview(target);
        r.application = venv.join(pkg).join("1");
        r.venv = Some(venv.to_path_buf());
        r
    }

    /// The venv as a whole, for the places that treat `application` as a
    /// boundary rather than a destination: the manifest store, and the prune
    /// that stops there on uninstall — so removing a package takes its `1`
    /// and its `<pkg>` dir with it.
    pub fn project_store(venv: &Path, target: &LvTarget) -> Roots {
        let mut r = Roots::labview(target);
        r.application = venv.to_path_buf();
        r.venv = Some(venv.to_path_buf());
        r
    }

    /// The venv these roots belong to, when they do.
    pub fn venv(&self) -> Option<&Path> {
        self.venv.as_deref()
    }

    /// What kind of place a `Target Dir` names, without resolving it.
    pub fn classify(target_dir: &str) -> Result<TokenClass> {
        let Some((tok, _)) = target_dir.split_once('>') else {
            bail!("malformed Target Dir: {target_dir:?}");
        };
        Ok(match format!("{tok}>").as_str() {
            "<temp>" => TokenClass::Temp,
            t if t.starts_with("<OS ") => TokenClass::Os,
            _ => TokenClass::LabView,
        })
    }

    /// Resolve a `Target Dir` such as `<menus>/Categories`.
    pub fn expand(&self, target_dir: &str) -> Result<PathBuf> {
        let Some((tok, rest)) = target_dir.split_once('>') else {
            bail!("malformed Target Dir: {target_dir:?}");
        };
        let token = format!("{tok}>");
        let rest = rest.trim_start_matches(['/', '\\']);

        let app = &self.application;
        let base = match token.as_str() {
            "<application>" => app.clone(),
            "<vi.lib>" => app.join("vi.lib"),
            "<user.lib>" => app.join("user.lib"),
            "<instr.lib>" => app.join("instr.lib"),
            "<menus>" => app.join("menus"),
            "<resource>" => app.join("resource"),
            "<help>" => app.join("help"),
            "<project>" => app.join("project"),
            "<templates>" => app.join("templates"),
            "<examples>" => app.join("examples"),
            "<fonts>" => app.join("fonts"),
            "<temp>" => self.temp.clone(),
            "<OS Public Application Data>" => self.program_data.clone(),
            "<OS Application Files>" => self.program_files.clone(),
            "<OS Public Documents>" => self.public_documents.clone(),
            "<OS User Documents>" => self.user_documents.clone(),
            "<OS User Desktop>" => self.user_desktop.clone(),
            "<OS User Application Data>" => self.user_appdata.clone(),
            "<OS Boot Volume Root>" => self.boot_volume.clone(),
            "<OS System Core Libraries>" => self.system_core.clone(),
            other => bail!("unknown Target Dir token {other:?} (from {target_dir:?})"),
        };

        // Specs spell the tail with either separator, and only Windows takes
        // both: on Linux `addons\Foo` would be one directory of that name.
        Ok(rest.split(['/', '\\']).filter(|c| !c.is_empty()).fold(base, |p, c| p.join(c)))
    }

    /// Manifests live beside the thing they describe: inside the venv or the
    /// sandbox, and for a real install in machine-wide state keyed by target —
    /// `%ProgramData%` on Windows, `/var/lib` on Linux.
    pub fn store_dir(&self) -> PathBuf {
        #[cfg(windows)]
        let machine = env_path("ProgramData", "C:\\ProgramData");
        #[cfg(target_os = "linux")]
        let machine = PathBuf::from("/var/lib");
        match (&self.venv, &self.scratch, &self.target) {
            (Some(v), _, _) => v.join(".lvpm").join("installed"),
            (None, Some(p), _) => p.join(".lvpm").join("installed"),
            (None, None, Some(t)) => machine
                .join("lvpm")
                .join(t.key())
                .join("installed"),
            (None, None, None) => PathBuf::from(".lvpm/installed"),
        }
    }

    /// Is this directory a root that packages share, rather than one package's
    /// own folder?
    ///
    /// `vi.lib` holds every add-on ever installed, and the install dir holds
    /// LabVIEW itself. Anything that walks a directory tree — relinking, say —
    /// has to know the difference, because handed one of these it would work on
    /// code no install of ours ever touched.
    pub fn is_shared_root(&self, dir: &Path) -> bool {
        // Above the install dir is shared by definition.
        if self.application.starts_with(dir) {
            return true;
        }
        [
            "<vi.lib>",
            "<user.lib>",
            "<instr.lib>",
            "<menus>",
            "<resource>",
            "<help>",
            "<project>",
            "<templates>",
            "<examples>",
            "<fonts>",
            "<temp>",
            "<OS Public Application Data>",
            "<OS Application Files>",
            "<OS Public Documents>",
            "<OS User Documents>",
            "<OS User Desktop>",
            "<OS User Application Data>",
            "<OS Boot Volume Root>",
            "<OS System Core Libraries>",
        ]
        .iter()
        .filter_map(|t| self.expand(t).ok())
        .any(|root| root == dir)
    }

    /// Guard against a package writing outside the sandbox, or the venv.
    pub fn check_contained(&self, p: &Path) -> Result<()> {
        let bound = match (&self.scratch, &self.venv) {
            (Some(root), _) => Some((root, "the scratch prefix")),
            (None, Some(root)) => Some((root, "the venv")),
            (None, None) => None,
        };
        if let Some((root, what)) = bound
            && !p.starts_with(root)
        {
            bail!("refusing to write outside {what}: {}", p.display());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scratch_keeps_every_token_inside_the_prefix() {
        let r = Roots::scratch(Path::new("/tmp/box"));
        for tok in [
            "<application>",
            "<menus>/Categories",
            "<OS Public Application Data>",
            "<OS Boot Volume Root>",
            "<temp>",
        ] {
            let p = r.expand(tok).unwrap();
            assert!(p.starts_with("/tmp/box"), "{tok} escaped: {}", p.display());
            r.check_contained(&p).unwrap();
        }
    }

    /// An install dir and a repo, as each platform spells them.
    #[cfg(windows)]
    const LV: &str = r"C:\Program Files\National Instruments\LabVIEW 2026";
    #[cfg(not(windows))]
    const LV: &str = "/usr/local/natinst/LabVIEW-2026-64";
    #[cfg(windows)]
    const REPO: &str = r"C:\repo";
    #[cfg(not(windows))]
    const REPO: &str = "/home/dev/repo";

    #[test]
    fn labview_roots_hang_off_the_install_dir() {
        let t = LvTarget { version: 26.3, bitness: 64, path: PathBuf::from(LV) };
        assert_eq!(t.year(), 2026);
        assert_eq!(t.key(), "LabVIEW-2026-64bit");
        let r = Roots::labview(&t);
        assert_eq!(
            r.expand("<vi.lib>/addons/HSE").unwrap(),
            Path::new(LV).join("vi.lib").join("addons").join("HSE")
        );
        assert_eq!(
            r.expand("<menus>/Categories").unwrap(),
            Path::new(LV).join("menus").join("Categories")
        );
    }

    #[test]
    fn project_roots_put_labview_tokens_in_the_addon_and_leave_machine_roots_alone() {
        let t = LvTarget { version: 26.3, bitness: 64, path: PathBuf::from(LV) };
        let venv = &Path::new(REPO).join(".project");
        let r = Roots::project(venv, &t, "oglib_error");
        let addon = venv.join("oglib_error").join("1");

        assert_eq!(
            r.expand("<vi.lib>/_OpenG.lib/error").unwrap(),
            addon.join("vi.lib").join("_OpenG.lib").join("error")
        );
        assert_eq!(r.expand("<application>").unwrap(), addon);
        assert!(!r.expand("<OS Public Application Data>").unwrap().starts_with(REPO));
        assert!(!r.expand("<temp>").unwrap().starts_with(REPO));

        // Nothing LabVIEW-class may leave the venv; machine roots are not
        // written to at all in venv mode, and the guard is what says so.
        r.check_contained(&r.expand("<menus>/Categories").unwrap()).unwrap();
        assert!(r.check_contained(&r.expand("<temp>").unwrap()).is_err());

        assert_eq!(r.store_dir(), venv.join(".lvpm").join("installed"));
        assert_eq!(Roots::project_store(venv, &t).store_dir(), r.store_dir());
        assert_eq!(&Roots::project_store(venv, &t).application, venv);
        assert_eq!(r.venv(), Some(venv.as_path()));
        assert!(r.target.is_some());
    }

    /// On Linux the `<OS ...>` tokens land where LabVIEW's Get System
    /// Directory.vi says, and a global install's manifests in /var/lib — not
    /// in a `C:\ProgramData` relative to the working directory, which is what
    /// the Windows fallbacks amount to there.
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_roots_are_the_ones_labview_reports() {
        let t = LvTarget { version: 26.3, bitness: 64, path: PathBuf::from(LV) };
        let r = Roots::labview(&t);
        let home = env_path("HOME", "/root");
        assert_eq!(r.expand("<OS User Documents>/x").unwrap(), home.join("Documents/x"));
        assert_eq!(r.expand("<OS User Application Data>").unwrap(), home);
        assert_eq!(r.expand("<OS Public Application Data>").unwrap(), Path::new("/usr/local"));
        assert_eq!(r.expand("<OS Boot Volume Root>/ci").unwrap(), Path::new("/ci"));
        assert_eq!(r.expand("<OS System Core Libraries>").unwrap(), Path::new("/usr/lib"));
        assert_eq!(r.store_dir(), Path::new("/var/lib/lvpm/LabVIEW-2026-64bit/installed"));
        assert_eq!(t.exe(), Path::new(LV).join("labview"));
        assert_eq!(t.ini(), home.join("natinst/.config/LabVIEW-2026/labview.conf"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_version_comes_from_the_uninstall_script() {
        let dir = std::env::temp_dir().join(format!("lvpm-target-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("readme")).unwrap();
        assert_eq!(linux_version(&dir), None);
        std::fs::write(
            dir.join("readme/UNINSTALL"),
            "# LabVIEW 2026 Q3 uninstallation script.\nLV_MAJOR_VER=26\nLV_MINOR_VER=3\nLV_UPDATE_VER=0\n",
        )
        .unwrap();
        assert_eq!(linux_version(&dir), Some(26.3));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_target_dir_may_use_either_separator() {
        let r = Roots::scratch(Path::new("/tmp/box"));
        let want = Path::new("/tmp/box/LabVIEW").join("vi.lib").join("addons").join("Foo");
        assert_eq!(r.expand("<vi.lib>/addons/Foo").unwrap(), want);
        assert_eq!(r.expand("<vi.lib>\\addons\\Foo\\").unwrap(), want);
    }

    #[test]
    fn classify_sorts_tokens_by_where_they_point() {
        assert_eq!(Roots::classify("<vi.lib>/addons/Foo").unwrap(), TokenClass::LabView);
        assert_eq!(Roots::classify("<application>").unwrap(), TokenClass::LabView);
        assert_eq!(Roots::classify("<menus>").unwrap(), TokenClass::LabView);
        assert_eq!(Roots::classify("<temp>").unwrap(), TokenClass::Temp);
        assert_eq!(Roots::classify("<OS Public Application Data>/x").unwrap(), TokenClass::Os);
        assert_eq!(Roots::classify("<OS System Core Libraries>").unwrap(), TokenClass::Os);
        assert!(Roots::classify("nonsense").is_err());
    }
}
