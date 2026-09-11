//! LabVIEW targets, and the directory roots that `Target Dir` tokens resolve to.
//!
//! A scratch prefix and a real LabVIEW installation differ only in where the
//! roots point, so both go through `Roots`. That keeps the installer honest:
//! the code path exercised against a sandbox is the same one that writes into
//! `C:\Program Files\National Instruments\LabVIEW 2026`.

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
}

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
            if !path.join("LabVIEW.exe").exists() {
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

#[cfg(not(windows))]
pub fn detect() -> Result<Vec<LvTarget>> {
    // LabVIEW on Linux lives under /usr/local/natinst/LabVIEW-<year>-64
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir("/usr/local/natinst") {
        for e in rd.flatten() {
            let p = e.path();
            let n = p.file_name().unwrap_or_default().to_string_lossy().to_string();
            if let Some(rest) = n.strip_prefix("LabVIEW-")
                && let Some(year) = rest.split('-').next().and_then(|y| y.parse::<u32>().ok())
            {
                let bitness = if n.ends_with("-64") { 64 } else { 32 };
                out.push(LvTarget { version: (year - 2000) as f64, bitness, path: p });
            }
        }
    }
    out.sort_by(|a, b| b.version.partial_cmp(&a.version).unwrap_or(std::cmp::Ordering::Equal));
    Ok(out)
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

        Ok(if rest.is_empty() { base } else { base.join(rest) })
    }

    /// Manifests live beside the thing they describe: inside the venv or the
    /// sandbox, in ProgramData keyed by target for a real install.
    pub fn store_dir(&self) -> PathBuf {
        match (&self.venv, &self.scratch, &self.target) {
            (Some(v), _, _) => v.join(".lvpm").join("installed"),
            (None, Some(p), _) => p.join(".lvpm").join("installed"),
            (None, None, Some(t)) => env_path("ProgramData", "C:\\ProgramData")
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

    #[test]
    fn labview_roots_hang_off_the_install_dir() {
        let t = LvTarget {
            version: 26.3,
            bitness: 64,
            path: PathBuf::from(r"C:\Program Files\National Instruments\LabVIEW 2026"),
        };
        assert_eq!(t.year(), 2026);
        assert_eq!(t.key(), "LabVIEW-2026-64bit");
        let r = Roots::labview(&t);
        assert_eq!(
            r.expand("<vi.lib>/addons/HSE").unwrap(),
            Path::new(r"C:\Program Files\National Instruments\LabVIEW 2026\vi.lib\addons\HSE")
        );
        assert_eq!(
            r.expand("<menus>/Categories").unwrap(),
            Path::new(r"C:\Program Files\National Instruments\LabVIEW 2026\menus\Categories")
        );
    }

    #[test]
    fn project_roots_put_labview_tokens_in_the_addon_and_leave_machine_roots_alone() {
        let t = LvTarget { version: 26.3, bitness: 64, path: PathBuf::from(r"C:\LV2026") };
        let venv = Path::new(r"C:\repo\.project");
        let r = Roots::project(venv, &t, "oglib_error");

        assert_eq!(
            r.expand("<vi.lib>/_OpenG.lib/error").unwrap(),
            Path::new(r"C:\repo\.project\oglib_error\1\vi.lib\_OpenG.lib\error")
        );
        assert_eq!(r.expand("<application>").unwrap(), Path::new(r"C:\repo\.project\oglib_error\1"));
        assert!(!r.expand("<OS Public Application Data>").unwrap().starts_with(r"C:\repo"));
        assert!(!r.expand("<temp>").unwrap().starts_with(r"C:\repo"));

        // Nothing LabVIEW-class may leave the venv; machine roots are not
        // written to at all in venv mode, and the guard is what says so.
        r.check_contained(&r.expand("<menus>/Categories").unwrap()).unwrap();
        assert!(r.check_contained(&r.expand("<temp>").unwrap()).is_err());

        assert_eq!(r.store_dir(), Path::new(r"C:\repo\.project\.lvpm\installed"));
        assert_eq!(Roots::project_store(venv, &t).store_dir(), r.store_dir());
        assert_eq!(Roots::project_store(venv, &t).application, venv);
        assert_eq!(r.venv(), Some(venv));
        assert!(r.target.is_some());
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
