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
            target: Some(t.clone()),
        }
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

    /// Manifests live beside the thing they describe: inside the sandbox for a
    /// scratch install, in ProgramData keyed by target for a real one.
    pub fn store_dir(&self) -> PathBuf {
        match (&self.scratch, &self.target) {
            (Some(p), _) => p.join(".lvpm").join("installed"),
            (None, Some(t)) => env_path("ProgramData", "C:\\ProgramData")
                .join("lvpm")
                .join(t.key())
                .join("installed"),
            (None, None) => PathBuf::from(".lvpm/installed"),
        }
    }

    /// Guard against a package writing outside the sandbox in scratch mode.
    pub fn check_contained(&self, p: &Path) -> Result<()> {
        if let Some(root) = &self.scratch
            && !p.starts_with(root)
        {
            bail!("refusing to write outside the scratch prefix: {}", p.display());
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
}
