//! Making a newly installed package visible: the palettes and the Tools menu.
//!
//! Copying a `.vip`'s files is not the end of an install even once they are
//! relinked. A package that ships palette entries drops
//! `functions_<package>.mnu` files into `<menus>/Categories/<Category>`, and a
//! package with a menu-launch VI drops it under `<project>`. LabVIEW reads
//! both trees when it starts and caches them, so until it is told otherwise a
//! freshly installed package has no palette entry and no **Tools** entry — the
//! install looks like it did nothing. 
//!
//! Two Application-class methods do the work, `Palettes:Refresh` and
//! `Menus:Refresh`, and **neither can be invoked from here.** They resolve
//! over `AppDoMethod` — the ids are right, see `docs/vi-server-protocol.md` —
//! and LabVIEW answers **1032, VI Server access denied**, because NI marks
//! both as not remotely accessible and every TCP client counts as remote.
//! `Bring To Front`, which is marked accessible, succeeds on the same
//! connection, so this is the policy and not our encoding.
//!
//! What works is the indirection LabVIEW itself provides: a VI *inside* that
//! LabVIEW invoking the method locally. Both such VIs already ship with
//! LabVIEW, each a single Invoke Node with no controls, so there is nothing to
//! author and nothing to keep in step with a release:
//!
//! | | VI |
//! |---|---|
//! | palettes | `vi.lib\Palette API\Refresh Palettes.vi` |
//! | menus | `resource\plugins\PopupMenus\support\Refresh Menus.vi` |
//!
//! Present in every install checked (2015, 2025, 2026). So this module is only
//! plumbing: find the two VIs, run them, report what happened.

use crate::target::LvTarget;
use crate::viserver::{self, Connection};
use anyhow::{Context, Result};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// What to refresh, and the VI in the LabVIEW installation that does it.
///
/// Paths relative to the install directory. `Refresh Palettes.vi` is public,
/// documented Palette API; `Refresh Menus.vi` is an internal support VI of the
/// right-click-menu plug-in framework that happens to be the only shipping
/// wrapper around `Menus:Refresh` — a fair reason to expect its path to move
/// one day, which is why a missing VI is reported rather than fatal.
const REFRESH_VIS: [(&str, &str); 2] = [
    ("palettes", r"vi.lib\Palette API\Refresh Palettes.vi"),
    ("menus", r"resource\plugins\PopupMenus\support\Refresh Menus.vi"),
];

/// One refresh: what it was, and how it went.
pub struct Outcome {
    pub what: &'static str,
    pub result: Result<Duration>,
}

/// Refresh the palettes and the File/Tools/Help menus of a running LabVIEW.
///
/// One connection for both VIs. Each is a separate failure: a missing or
/// broken `Refresh Menus.vi` must not cost the caller its palette refresh.
/// The connection itself failing is the one error that fails the call, since
/// then nothing was refreshed at all.
pub fn run(target: &LvTarget, timeout: Duration) -> Result<Vec<Outcome>> {
    let port = viserver::ensure_vi_server(target, Duration::from_secs(120))?;
    let mut conn = Connection::connect("127.0.0.1", port, timeout)?;

    let mut out = Vec::with_capacity(REFRESH_VIS.len());
    for (what, rel) in REFRESH_VIS {
        out.push(Outcome { what, result: run_one(&mut conn, target.path.join(rel)) });
    }
    conn.close();
    Ok(out)
}

/// Load one refresh VI and run it to completion.
///
/// Both take a couple of seconds — LabVIEW is rebuilding a tree, and the reply
/// only arrives when the VI stops — so the connection's read timeout has to
/// cover that, not just a round trip.
fn run_one(conn: &mut Connection, vi: PathBuf) -> Result<Duration> {
    if !vi.is_file() {
        anyhow::bail!("{} does not exist in this LabVIEW", vi.display());
    }
    let t0 = Instant::now();
    let r = conn.open_vi_reference(&vi).with_context(|| format!("opening {}", vi.display()))?;
    let run = conn.run_vi(r);
    let _ = conn.release(r);
    run.map(|()| t0.elapsed())
}
