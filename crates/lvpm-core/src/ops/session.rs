//! [`InstallSession`]: what an install or an uninstall carries from its first
//! package to its last.
//!
//! One HTTP client and cache for the downloads, one VI Server connection for
//! the hooks, and the work that waits for the end: the folders to relink and
//! the PostInstall hooks to run once the files are in place. [`finish`] spends
//! it, and an uninstall feeds the same session removals instead of installs.
//!
//! [`finish`]: super::install::finish

use super::env::{Destination, OpEnv};
use super::hooks::{HookCall, PostInstallHook, run_reported_hook};
use super::resave::Resaver;
use super::staging::Staging;
use crate::cache::{Cache, Stored};
use crate::cancel::CancelToken;
use crate::fetch;
use crate::index::Entry;
use crate::target::{LvTarget, Roots};
use crate::venv::Venv;
use crate::viserver::Connection;
use anyhow::Result;
use reqwest::blocking::Client;
use std::path::PathBuf;

/// What fetching an archive needs, opened when the first archive is wanted:
/// an uninstall never gets here. Locking has no destination and so no session;
/// it fetches through the same thing, as `info` does on a cache miss.
pub(super) struct Downloads {
    client: Client,
    cache: Cache,
    cancel: CancelToken,
}

impl Downloads {
    pub(super) fn open(env: &OpEnv) -> Result<Downloads> {
        Ok(Downloads {
            client: Client::builder()
                .user_agent(concat!("lvpm/", env!("CARGO_PKG_VERSION")))
                .build()?,
            cache: Cache::open(&env.config.cache)?,
            cancel: env.cancel.clone(),
        })
    }

    /// The archive's bytes, from the cache, a local file or the entry's feed.
    /// [`Cancelled`](crate::errors::Cancelled) when the environment's token is.
    pub(super) fn fetch(&self, entry: &Entry) -> Result<Vec<u8>> {
        fetch::package(&self.cache, &self.client, entry, &self.cancel)
    }

    /// What the cache holds under this SHA-256, without removing anything.
    pub(super) fn stored(&self, sha256: &str) -> Stored {
        self.cache.lookup(sha256)
    }

    /// Keep bytes in the cache under their own SHA-256, so that a later read
    /// of that SHA-256 finds them.
    pub(super) fn keep(&self, bytes: &[u8]) -> Result<String> {
        self.cache.store(bytes)
    }
}

pub struct InstallSession {
    dest: Destination,
    store: Roots,
    downloads: Option<Downloads>,
    /// Opened by the first hook and shared by the rest, so a LabVIEW is asked
    /// once however many packages have hooks.
    hook_connection: Option<Connection>,
    /// Per package, in install order: the folders its files landed in.
    /// Relinking happens after every package is on disk, so no package can be
    /// relinked against a dependency that is not there yet.
    pub(super) relink_work: Vec<(String, Vec<PathBuf>)>,
    /// PostInstall hooks extracted so far, executed only after the relink
    /// pass has made them runnable.
    pub(super) post_install_hooks: Vec<PostInstallHook>,
    /// The LabVIEW a venv install resaves its staged packages in.
    pub(super) resaver: Resaver,
}

impl InstallSession {
    pub fn new(dest: Destination) -> InstallSession {
        InstallSession {
            store: dest.store(),
            dest,
            downloads: None,
            hook_connection: None,
            relink_work: Vec::new(),
            post_install_hooks: Vec::new(),
            resaver: Resaver::default(),
        }
    }

    pub fn destination(&self) -> &Destination {
        &self.dest
    }

    /// The store is one per destination: what reads or removes installed
    /// packages goes through it.
    pub fn store(&self) -> &Roots {
        &self.store
    }

    /// The LabVIEW installation the destination belongs to, if it has one: a
    /// scratch tree has none.
    pub fn target(&self) -> Option<&LvTarget> {
        self.store.target.as_ref()
    }

    pub fn venv(&self) -> Option<&Venv> {
        match &self.dest {
            Destination::Venv(v) => Some(v),
            _ => None,
        }
    }

    /// Where a package's own files go. In a venv every package is an addon of
    /// its own; elsewhere the store is the destination.
    pub fn roots_for(&self, package: &str) -> Result<Roots> {
        match self.venv() {
            Some(v) => v.roots_for_pkg(package),
            None => Ok(self.store.clone()),
        }
    }

    /// The archive's bytes, from the cache, a local file or the entry's feed.
    pub fn fetch(&mut self, env: &OpEnv, entry: &Entry) -> Result<Vec<u8>> {
        if self.downloads.is_none() {
            self.downloads = Some(Downloads::open(env)?);
        }
        self.downloads.as_ref().expect("opened above").fetch(entry)
    }

    /// Run one hook on the session's VI Server connection, opening it if no
    /// hook has yet, and report how it went. Whether it ran to the end.
    pub fn run_hook(&mut self, env: &OpEnv, target: &LvTarget, call: &HookCall) -> bool {
        run_reported_hook(env, &mut self.hook_connection, target, call)
    }

    /// Resave a staged venv package; anywhere else there is nothing to do.
    pub(super) fn resave(&mut self, env: &OpEnv, staging: &Staging, package: &str) {
        if let Destination::Venv(v) = &self.dest {
            self.resaver
                .resave(v, env.headless, env.sink, staging, package);
        }
    }

    /// Let go of the hook connection: the hooks that follow open their own.
    pub fn close_hook_connection(&mut self) {
        if let Some(connection) = self.hook_connection.take() {
            connection.close();
        }
    }
}
