//! Questions about what lvpm knows, answered as data: the front end decides
//! how many rows to show and what to call them.

use super::env::OpEnv;
use super::session::Downloads;
use crate::cache::Cache;
use crate::config;
use crate::events::Event;
use crate::fetch;
use crate::index::{self, Entry, Import};
use crate::install::{self, Manifest};
use crate::paths;
use crate::project::{PackageArg, same_name};
use crate::solve;
use crate::spec::Spec;
use crate::target::LvTarget;
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::path::Path;

/// The LabVIEW installations detected on this machine, newest first.
pub fn targets<'e>(env: &'e OpEnv) -> &'e [LvTarget] {
    &env.targets
}

/// The LabVIEW year `entry` needs when `target` is older than that, so that
/// install and lock would refuse it there; `None` when it fits.
pub fn needs_newer_labview(entry: &Entry, target: &LvTarget) -> Option<u32> {
    solve::gated(entry, target.version).map(|gated| gated.needs_year)
}

/// The version of one package that install would resolve with no floor:
/// the newest of `versions` (newest first) that `target` can take.
pub fn resolvable<'e>(versions: &'e [Entry], target: Option<&LvTarget>) -> Option<&'e Entry> {
    versions
        .iter()
        .find(|e| target.is_none_or(|t| e.fits_labview(t.version)))
}

/// Every package whose name or display name contains `query`, newest version
/// of each, by name. All of them: a front end that shows fewer says so itself.
pub fn search(env: &OpEnv, query: &str) -> Result<Vec<Entry>> {
    let manifest = env.manifest_for(None, None)?;
    let index = env.load_index(manifest.as_ref())?;
    Ok(index.search(query).into_iter().cloned().collect())
}

/// Every version of every package the sources list, by name and then newest
/// first, narrowed to names or display names containing `query` in any case.
/// A source that cannot be read is named in `failed` and the others are still
/// listed; when every source fails and nothing is imported, so does this.
pub fn catalog(env: &OpEnv, query: Option<&str>) -> Result<Catalog> {
    let manifest = env.manifest_for(None, None)?;
    let mut failed = Vec::new();
    let loaded = env.load_index_each(manifest.as_ref(), |source, error| {
        failed.push((source.to_string(), format!("{error:#}")));
        Ok(())
    })?;
    let all_failed = !failed.is_empty() && failed.len() == loaded.sources.len();
    if all_failed && loaded.index.entries.is_empty() {
        let why: Vec<String> = failed.iter().map(|(s, e)| format!("{s}: {e}")).collect();
        anyhow::bail!("no package source could be read:\n  {}", why.join("\n  "));
    }
    let mut entries: Vec<Entry> = loaded
        .index
        .matching(query.unwrap_or(""))
        .cloned()
        .collect();
    // Names equal under `name_key` (ASCII case) are one package, so they sort together.
    entries.sort_by_cached_key(|e| {
        (
            crate::project::name_key(&e.name),
            std::cmp::Reverse(e.version.clone()),
        )
    });
    Ok(Catalog {
        entries,
        sources: loaded.sources,
        failed,
    })
}

/// The answer to [`catalog`].
#[derive(Debug, Serialize)]
pub struct Catalog {
    pub entries: Vec<Entry>,
    /// Every source asked: each local folder, then each feed's index URL.
    pub sources: Vec<String>,
    /// The sources that could not be read, with why.
    pub failed: Vec<(String, String)>,
}

/// The details of one version of a package: `name` gives the newest version,
/// `name@constraint` the newest the constraint admits, read as `install` reads
/// it (`1.0`, `>=1.0`, `*`), and an error when none does. Its archive is read
/// from the cache, or downloaded into it, so no destination and no LabVIEW is
/// needed.
pub fn info(env: &OpEnv, package: &str) -> Result<PackageInfo> {
    let arg = PackageArg::parse(package)?;
    let constraint = arg.read_constraint()?;
    let catalog = catalog(env, Some(&arg.name))?;
    let versions: Vec<Entry> = catalog
        .entries
        .into_iter()
        .filter(|e| same_name(&e.name, &arg.name))
        .collect();
    // A source that could not be read may hold what was asked for.
    let unread: String = catalog
        .failed
        .iter()
        .map(|(source, why)| format!("\ncould not read {source}: {why}"))
        .collect();
    if versions.is_empty() {
        bail!(
            "no package named {:?} in:\n  {}{unread}",
            arg.name,
            catalog.sources.join("\n  ")
        );
    }
    // Newest first, so the first admitted is the newest.
    let entry = versions
        .iter()
        .find(|e| constraint.admits(&e.version))
        .with_context(|| {
            let known: Vec<String> = versions.iter().map(|e| e.version.to_string()).collect();
            let wanted = arg.constraint.as_deref().unwrap_or_default();
            format!(
                "{} has no version {wanted}; it has {}{unread}",
                arg.name,
                known.join(", ")
            )
        })?
        .clone();
    let what = format!("{} {}", entry.name, entry.version);
    let bytes = archive_bytes(env, &entry)
        .with_context(|| format!("could not fetch {what} from {}", entry.source))?;
    let spec = install::open_archive(bytes)
        .and_then(|mut zip| install::read_spec(&mut zip))
        .with_context(|| format!("the archive of {what} could not be read"))?;
    Ok(PackageInfo {
        entry,
        versions,
        spec,
        failed: catalog.failed,
    })
}

/// The archive of `entry`, downloaded into the cache unless it is on hand;
/// a download says so first.
fn archive_bytes(env: &OpEnv, entry: &Entry) -> Result<Vec<u8>> {
    // No HTTP client is built when the cache already has it.
    if let Some(bytes) = fetch::on_hand(&Cache::open(&env.config.cache)?, entry)? {
        return Ok(bytes);
    }
    env.sink.emit(Event::ArchiveFetching {
        name: entry.name.clone(),
        version: entry.version.to_string(),
    });
    Downloads::open(env)?.fetch(entry)
}

/// The answer to [`info`].
#[derive(Debug, Serialize)]
pub struct PackageInfo {
    /// The version shown: its requirements, minimum LabVIEW and source.
    pub entry: Entry,
    /// Every version the sources list, newest first, each with its source.
    pub versions: Vec<Entry>,
    /// The shown version's spec: its description fields and file groups.
    pub spec: Spec,
    /// The sources that could not be read, with why, as in [`Catalog`].
    pub failed: Vec<(String, String)>,
}

/// What lvpm has installed into the selected destination.
pub fn list(env: &OpEnv) -> Result<Vec<Manifest>> {
    install::list_installed(&env.destination()?.store())
}

/// The effective value of a setting, one string per value.
pub fn config_get(env: &OpEnv, key: &str) -> Result<Vec<String>> {
    config::get(&env.config, key)
}

/// Change a user-level setting. Writes the user's config file; `env.config`
/// is what the command started with and is not updated. A relative path is
/// taken from the directory the command was run from.
pub fn config_set(env: &OpEnv, key: &str, values: &[String]) -> Result<()> {
    config::set(key, values, &env.cwd)
}

/// Import a package archive, or the packages directly inside a directory, into
/// the shared cache. A relative `path` is taken from the directory the command
/// was run from.
pub fn cache_add(env: &OpEnv, path: &Path) -> Result<Vec<Import>> {
    index::cache_add(
        &env.config.cache,
        &paths::resolve(&env.cwd, path),
        env.refresh,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::events::NullSink;
    use std::path::PathBuf;

    fn env_with(cache: &str, venv_dir: &str, targets: Vec<LvTarget>) -> OpEnv<'static> {
        let config = Config {
            cache: PathBuf::from(cache),
            local_sources: vec![PathBuf::from("packages-a"), PathBuf::from("packages-b")],
            venv_dir: PathBuf::from(venv_dir),
        };
        OpEnv::new(&NullSink, config, targets, PathBuf::from("."))
    }

    #[test]
    fn the_detected_installations_are_the_ones_the_environment_was_given() {
        let labview = LvTarget {
            version: 26.3,
            bitness: 64,
            path: PathBuf::from("LabVIEW 2026"),
        };
        let env = env_with("cache", ".lv-venv", vec![labview.clone()]);
        assert_eq!(targets(&env).len(), 1);
        assert_eq!(targets(&env)[0].path, labview.path);
        assert!(targets(&env_with("cache", ".lv-venv", Vec::new())).is_empty());
    }

    #[test]
    fn a_package_needs_a_newer_labview_only_when_its_minimum_is_above_the_targets() {
        let labview_2025 = LvTarget {
            version: 25.0,
            bitness: 64,
            path: PathBuf::from("LabVIEW 2025"),
        };
        let needing = |lv_min: Option<f64>| Entry {
            name: "alpha".into(),
            version: crate::version::Version::parse("1.0"),
            url: String::new(),
            md5: None,
            display_name: None,
            requires: Vec::new(),
            lv_min,
            source: String::new(),
        };
        assert_eq!(
            needs_newer_labview(&needing(Some(99.0)), &labview_2025),
            Some(2099)
        );
        assert_eq!(
            needs_newer_labview(&needing(Some(25.0)), &labview_2025),
            None
        );
        assert_eq!(needs_newer_labview(&needing(None), &labview_2025), None);

        let newest_first = [needing(Some(99.0)), needing(Some(20.0))];
        assert_eq!(
            resolvable(&newest_first, Some(&labview_2025)).and_then(|e| e.lv_min),
            Some(20.0),
            "the newest version the LabVIEW can take"
        );
        assert_eq!(
            resolvable(&newest_first, None).and_then(|e| e.lv_min),
            Some(99.0),
            "no LabVIEW gates nothing"
        );
        assert!(resolvable(&newest_first[..1], Some(&labview_2025)).is_none());
    }

    #[test]
    fn cache_add_takes_a_relative_path_from_the_environments_directory_not_the_processs() {
        use std::io::Write;

        let cwd = super::super::testing::scratch_dir("cache-add-cwd");
        std::fs::create_dir_all(cwd.join("pkgs")).unwrap();
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        writer
            .start_file("spec", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer
            .write_all(b"[Package]\nName=alpha\nVersion=1.0\n")
            .unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        std::fs::write(cwd.join("pkgs").join("alpha-1.0.vip"), bytes).unwrap();

        let config = Config {
            cache: cwd.join("cache"),
            local_sources: Vec::new(),
            venv_dir: PathBuf::from(".lv-venv"),
        };
        let env = OpEnv::new(&NullSink, config, Vec::new(), cwd.clone());
        let imports = cache_add(&env, Path::new("pkgs")).unwrap();
        assert_eq!(imports.len(), 1);
        assert_eq!(imports[0].path, cwd.join("pkgs").join("alpha-1.0.vip"));
        let _ = std::fs::remove_dir_all(&cwd);
    }

    /// A project that reads only `folders`, each holding (name, version,
    /// display name) packages, in a scratch directory the caller removes.
    fn catalog_env(tag: &str, folders: &[&[(&str, &str, &str)]]) -> (PathBuf, OpEnv<'static>) {
        use std::io::Write;

        let cwd = super::super::testing::scratch_dir(tag);
        let manifest = "[project]\nlabview = \">=2024\"\n\n[sources]\ndefaults = false\n";
        std::fs::write(cwd.join("lvpm.toml"), manifest).unwrap();
        let mut local_sources = Vec::new();
        for (i, packages) in folders.iter().enumerate() {
            let dir = cwd.join(format!("folder-{i}"));
            std::fs::create_dir_all(&dir).unwrap();
            for (name, version, display) in packages.iter() {
                let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
                writer
                    .start_file("spec", zip::write::SimpleFileOptions::default())
                    .unwrap();
                let spec = format!(
                    "[Package]\nName={name}\nVersion={version}\nDisplay Name=\"{display}\"\n"
                );
                writer.write_all(spec.as_bytes()).unwrap();
                let bytes = writer.finish().unwrap().into_inner();
                std::fs::write(dir.join(format!("{name}-{version}.vip")), bytes).unwrap();
            }
            local_sources.push(dir);
        }
        let config = Config {
            cache: cwd.join("cache"),
            local_sources,
            venv_dir: PathBuf::from(".lv-venv"),
        };
        let env = OpEnv::new(&NullSink, config, Vec::new(), cwd.clone());
        (cwd, env)
    }

    fn rows(catalog: &Catalog) -> Vec<String> {
        let row = |e: &Entry| format!("{} {}", e.name, e.version);
        catalog.entries.iter().map(row).collect()
    }

    #[test]
    fn the_catalog_lists_each_name_from_its_first_source_by_name_then_newest_first() {
        let (cwd, env) = catalog_env(
            "catalog-all",
            &[
                &[
                    ("beta", "0.1", "Beta"),
                    ("alpha", "1.0", "Alpha"),
                    ("alpha", "1.5", "Alpha"),
                ],
                &[("alpha", "2.0", "Alpha"), ("gamma", "3.0", "Gamma")],
            ],
        );
        let catalog = catalog(&env, None).unwrap();
        // alpha 2.0 is shadowed: the first folder already lists alpha.
        assert_eq!(
            rows(&catalog),
            ["alpha 1.5", "alpha 1.0", "beta 0.1", "gamma 3.0"]
        );
        let folder = |i: usize| {
            cwd.join(format!("folder-{i}"))
                .to_string_lossy()
                .into_owned()
        };
        assert_eq!(catalog.entries[0].source, folder(0));
        assert_eq!(catalog.entries[3].source, folder(1));
        assert_eq!(catalog.sources, [folder(0), folder(1)]);
        assert!(catalog.failed.is_empty());
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn a_query_narrows_the_catalog_by_name_or_display_name_in_any_case() {
        let packages = [("alpha", "1.0", "Alpha Tools"), ("beta", "0.1", "Beta")];
        let (cwd, env) = catalog_env("catalog-query", &[&packages]);
        for query in ["alp", "ALP", "tools"] {
            let catalog = catalog(&env, Some(query)).unwrap();
            assert_eq!(rows(&catalog), ["alpha 1.0"], "{query}");
        }
        assert!(catalog(&env, Some("zzz")).unwrap().entries.is_empty());
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn a_source_that_cannot_be_read_is_named_and_the_others_are_still_listed() {
        let (cwd, mut env) = catalog_env("catalog-failed", &[&[("beta", "0.1", "Beta")]]);
        let missing = cwd.join("missing");
        env.config.local_sources.insert(0, missing.clone());
        let catalog = catalog(&env, None).unwrap();
        assert_eq!(rows(&catalog), ["beta 0.1"]);
        assert_eq!(catalog.failed.len(), 1);
        assert_eq!(catalog.failed[0].0, missing.to_string_lossy());
        assert!(catalog.failed[0].1.contains("is not a directory"));
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn one_unread_source_beside_an_empty_one_is_not_an_error() {
        let (cwd, mut env) = catalog_env("catalog-failed-empty", &[&[]]);
        env.config.local_sources.insert(0, cwd.join("missing"));
        let catalog = catalog(&env, None).unwrap();
        assert!(catalog.entries.is_empty());
        assert_eq!(catalog.failed.len(), 1);
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn names_sort_ignoring_case() {
        let packages = [("Beta", "0.1", "Beta"), ("alpha", "1.0", "Alpha")];
        let (cwd, env) = catalog_env("catalog-case", &[&packages]);
        assert_eq!(
            rows(&catalog(&env, None).unwrap()),
            ["alpha 1.0", "Beta 0.1"]
        );
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn names_differing_in_a_non_ascii_letters_case_are_two_packages() {
        let packages = [("ärger", "2.0", "Ärger"), ("Ärger", "1.0", "Ärger")];
        let (cwd, env) = catalog_env("catalog-non-ascii", &[&packages]);
        assert_eq!(
            rows(&catalog(&env, None).unwrap()),
            ["Ärger 1.0", "ärger 2.0"]
        );
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn imported_packages_are_listed_when_every_source_fails() {
        let (cwd, mut env) = catalog_env("catalog-imported", &[&[("alpha", "1.0", "Alpha")]]);
        cache_add(&env, &cwd.join("folder-0").join("alpha-1.0.vip")).unwrap();
        env.config.local_sources = vec![cwd.join("missing")];
        let catalog = catalog(&env, None).unwrap();
        assert_eq!(rows(&catalog), ["alpha 1.0"]);
        assert_eq!(catalog.entries[0].source, crate::index::CACHE_SOURCE);
        assert_eq!(catalog.failed.len(), 1);
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn the_catalog_fails_when_no_source_can_be_read() {
        let (cwd, mut env) = catalog_env("catalog-none-read", &[]);
        env.config.local_sources = vec![cwd.join("missing")];
        let why = catalog(&env, None).unwrap_err().to_string();
        assert!(why.starts_with("no package source could be read"), "{why}");
        assert!(why.contains("missing"), "{why}");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn an_empty_catalog_still_names_the_sources_it_asked() {
        let (cwd, env) = catalog_env("catalog-empty", &[&[]]);
        let catalog = catalog(&env, None).unwrap();
        assert!(catalog.entries.is_empty());
        assert_eq!(catalog.sources, [cwd.join("folder-0").to_string_lossy()]);
        assert!(catalog.failed.is_empty());
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn a_setting_is_read_from_the_environments_config_not_the_machines() {
        let env = env_with("the-cache", ".the-venv", Vec::new());
        assert_eq!(config_get(&env, "venv.dir").unwrap(), [".the-venv"]);
        assert_eq!(
            config_get(&env, "sources.local").unwrap(),
            ["packages-a", "packages-b"]
        );
        let cache = config_get(&env, "cache").unwrap();
        assert_eq!(cache, [PathBuf::from("the-cache").to_string_lossy()]);
        let why = config_get(&env, "nothing").unwrap_err().to_string();
        assert!(why.starts_with("unknown config key \"nothing\""), "{why}");
    }

    #[test]
    fn info_reads_the_newest_version_or_the_one_asked_for() {
        let packages = [("alpha", "1.0", "Old Alpha"), ("ALPHA", "2.0", "New Alpha")];
        let (cwd, env) = catalog_env("info-versions", &[&packages]);
        let newest = info(&env, "alpha").unwrap();
        assert_eq!(newest.spec.display_name.as_deref(), Some("New Alpha"));
        let versions: Vec<String> = newest
            .versions
            .iter()
            .map(|e| e.version.to_string())
            .collect();
        assert_eq!(versions, ["2.0", "1.0"]);
        let chosen = info(&env, "alpha@1.0").unwrap();
        assert_eq!(chosen.entry.version.to_string(), "1.0");
        assert_eq!(chosen.spec.display_name.as_deref(), Some("Old Alpha"));
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn info_reads_the_version_as_install_does() {
        let packages = [("alpha", "1.0", "Old Alpha"), ("alpha", "2.0", "New Alpha")];
        let (cwd, env) = catalog_env("info-constraint", &[&packages]);
        let version = |arg: &str| info(&env, arg).unwrap().entry.version.to_string();
        assert_eq!(version("alpha@>=1.0"), "2.0");
        assert_eq!(version("alpha@*"), "2.0");
        let why = info(&env, "alpha@>=3.0").unwrap_err().to_string();
        assert_eq!(why, "alpha has no version >=3.0; it has 2.0, 1.0");
        // Refused before any source is read, as `install` refuses it.
        for arg in ["alpha@<2.0", "nothing@<2.0"] {
            let why = info(&env, arg).unwrap_err().to_string();
            assert!(why.starts_with("unsupported version constraint"), "{why}");
        }
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn info_names_the_sources_it_could_not_read() {
        let (cwd, mut env) = catalog_env("info-unread", &[&[("alpha", "1.0", "Alpha")]]);
        let missing = cwd.join("missing");
        env.config.local_sources.push(missing.clone());
        let unread = format!("\ncould not read {}: ", missing.to_string_lossy());
        for arg in ["nothing", "alpha@9.9"] {
            let why = info(&env, arg).unwrap_err().to_string();
            assert!(why.contains(&unread), "{why}");
        }
        let found = info(&env, "alpha").unwrap();
        assert_eq!(found.failed.len(), 1);
        assert_eq!(found.failed[0].0, missing.to_string_lossy());
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn info_on_an_unknown_name_or_version_says_what_there_is() {
        let (cwd, env) = catalog_env("info-unknown", &[&[("alpha", "1.0", "Alpha")]]);
        let folder = cwd.join("folder-0").to_string_lossy().into_owned();
        let why = info(&env, "alp").unwrap_err().to_string();
        assert_eq!(why, format!("no package named \"alp\" in:\n  {folder}"));
        let why = info(&env, "alpha@9.9").unwrap_err().to_string();
        assert_eq!(why, "alpha has no version 9.9; it has 1.0");
        let why = info(&env, "alpha@").unwrap_err().to_string();
        assert_eq!(why, "a constraint is expected after \"@\" in \"alpha@\"");
        let _ = std::fs::remove_dir_all(&cwd);
    }
}
