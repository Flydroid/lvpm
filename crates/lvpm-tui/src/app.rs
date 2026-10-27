//! The TUI's state and its event loop.

use crate::browse::{Browse, Status};
use crate::destination::EnvBuilder;
use crate::detail::{Detail, Focus};
use crate::plan::Preview;
use crate::progress::Progress;
use crate::worker::{self, Done, Job, Outcome};
use crate::{keys, log, ui};
use lvpm_core::cancel::CancelToken;
use lvpm_core::install::Manifest;
use lvpm_core::ops::Destination;
use lvpm_core::ops::query::{Catalog, PackageInfo, needs_newer_labview, resolvable};
use lvpm_core::project::same_name;
use lvpm_core::target::LvTarget;
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{SystemTime, UNIX_EPOCH};

/// The catalog as far as it has been read.
#[derive(Debug)]
pub enum Listing {
    Loading,
    /// Core's error, whole: no source could be read.
    Failed(String),
    Loaded(Browse),
}

/// An install, from its plan to its result.
#[derive(Debug)]
pub struct Op {
    /// What is installed, `name@version`.
    pub package: String,
    /// What the dry run said the install does.
    pub preview: Preview,
    pub stage: Stage,
    /// Stops the dry run, then the install.
    cancel: CancelToken,
}

#[derive(Debug)]
pub enum Stage {
    /// The dry run is computing the plan.
    Planning,
    /// Core's error, whole.
    PlanFailed(String),
    Preview,
    /// A downgrade is asked about a second time.
    ConfirmDowngrade,
    /// The install runs, or has ended once `outcome` is set.
    Running(Progress),
}

/// A package version's details, by name and version.
pub type Details = HashMap<(String, String), Result<PackageInfo, String>>;

#[derive(Debug)]
pub struct App {
    running: bool,
    builder: EnvBuilder,
    /// Where installs go, or why there is nowhere.
    pub destination: Result<Destination, String>,
    pub listing: Listing,
    /// The detail view, open over the list.
    pub detail: Option<Detail>,
    /// Every version's details read since the catalog was, or why not.
    pub details: Details,
    /// What the last key could not do, shown until the next key.
    pub note: Option<String>,
    /// The running job and its result, when it comes. At most one job runs.
    pending: Option<(Job, Receiver<Done>)>,
    /// The install on screen, over the list or the detail view.
    pub op: Option<Op>,
    /// Each install's log lines this session, oldest first.
    pub log: Vec<Vec<String>>,
    /// The log is on screen, over everything else.
    pub showing_log: bool,
    /// Where each install's log lines are appended; `None` writes nothing.
    pub log_file: Option<PathBuf>,
    /// Each feed the catalog load asked, with how it answered so far.
    pub feeds: Vec<(String, String)>,
}

impl App {
    /// Resolve the destination and start reading the catalog.
    pub fn new(builder: EnvBuilder) -> App {
        let mut app = App {
            running: true,
            destination: builder.destination(),
            builder,
            listing: Listing::Loading,
            detail: None,
            details: Details::new(),
            note: None,
            pending: None,
            op: None,
            log: Vec::new(),
            showing_log: false,
            log_file: None,
            feeds: Vec::new(),
        };
        app.start(Job::Load { refresh: false });
        app
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    /// The feeds are being read again; the list shown is the last one read.
    pub fn reloading(&self) -> bool {
        let loading = matches!(self.pending, Some((Job::Load { .. }, _)));
        loading && matches!(self.listing, Listing::Loaded(_))
    }

    /// An install runs and has not ended.
    pub fn installing(&self) -> bool {
        let stage = self.op.as_ref().map(|op| &op.stage);
        matches!(stage, Some(Stage::Running(progress)) if progress.outcome.is_none())
    }

    fn target(&self) -> Option<&LvTarget> {
        self.destination.as_ref().ok().and_then(Destination::target)
    }

    fn start(&mut self, job: Job) {
        debug_assert!(self.pending.is_none(), "one job at a time");
        if matches!(job, Job::Load { .. }) {
            self.feeds.clear();
        }
        self.pending = Some((job.clone(), worker::spawn(&self.builder, job)));
    }

    /// Why a job cannot start now, if one is running.
    fn busy(&self) -> Option<String> {
        let why = match &self.pending.as_ref()?.0 {
            Job::Load { .. } => "the catalog is still loading",
            Job::Info { .. } => "the details are still loading",
            Job::Install { .. } => "the last plan is still stopping",
        };
        Some(why.into())
    }

    /// Read the details the detail view shows, unless they are read or a
    /// job is running: that job's end asks again. An install on screen
    /// waits too, so its run does not have to.
    fn fetch_wanted(&mut self) {
        let shown = self.running && self.op.is_none();
        let Some(detail) = self.detail.as_ref().filter(|_| shown) else {
            return;
        };
        let key = detail.key();
        if self.pending.is_none() && !self.details.contains_key(&key) {
            let (name, version) = key;
            self.start(Job::Info { name, version });
        }
    }

    fn finish(&mut self, done: Done) {
        // An event comes while its job runs on.
        if !matches!(done, Done::Event(_)) {
            self.pending = None;
        }
        match done {
            Done::Event(event) => self.event(&event),
            Done::Installed { dry_run, outcome } => self.installed(dry_run, outcome),
            Done::Loaded {
                destination,
                listing,
            } => {
                // The installed list was read from this destination, so the
                // header and the LabVIEW gate follow it.
                self.destination = destination;
                self.listed(listing);
            }
            Done::Info {
                name,
                version,
                info,
            } => {
                self.details.insert((name, version), info);
            }
        }
        self.fetch_wanted();
    }

    /// Take in one of the install's events: the plan or the progress. Those
    /// of a plan dropped with Esc have no op to go to.
    fn event(&mut self, event: &lvpm_core::events::Event) {
        if matches!(self.pending, Some((Job::Load { .. }, _))) {
            self.feed(event);
            return;
        }
        let Some(op) = &mut self.op else {
            return;
        };
        match &mut op.stage {
            Stage::Planning => op.preview.add(event),
            Stage::Running(progress) => {
                progress.add(event);
                let entry = self.log.last_mut();
                if let (Some(entry), Some(line)) = (entry, log::line(event)) {
                    entry.push(line);
                }
            }
            _ => {}
        }
    }

    /// Take in one of the catalog load's events: a feed asked or answered.
    fn feed(&mut self, event: &lvpm_core::events::Event) {
        use lvpm_core::events::{Event, FeedOutcome};
        match event {
            Event::FeedCheckStart { url, first_run } => {
                let state = if *first_run {
                    "downloading…"
                } else {
                    "checking…"
                };
                self.feeds.push((url.clone(), state.into()));
            }
            Event::FeedChecked {
                url,
                outcome,
                bytes,
                elapsed,
            } => {
                let seconds = elapsed.as_secs_f32();
                let state = match outcome {
                    FeedOutcome::Unchanged => format!("unchanged, {seconds:.1}s"),
                    FeedOutcome::Downloaded => {
                        format!("{:.1} MB in {seconds:.1}s", *bytes as f32 / 1_048_576.0)
                    }
                };
                if let Some(feed) = self.feeds.iter_mut().rev().find(|(u, _)| u == url) {
                    feed.1 = state;
                }
            }
            _ => {}
        }
    }

    fn installed(&mut self, dry_run: bool, outcome: Outcome) {
        let Some(op) = &mut self.op else {
            return;
        };
        match (dry_run, outcome) {
            (true, Outcome::Done) => op.stage = Stage::Preview,
            (true, Outcome::Failed(error)) => op.stage = Stage::PlanFailed(error),
            (true, Outcome::Cancelled) => self.op = None,
            (false, outcome) => {
                if let Some(entry) = self.log.last_mut() {
                    entry.extend(log::outcome(&outcome));
                }
                if let Stage::Running(progress) = &mut op.stage {
                    progress.outcome = Some(outcome);
                }
                self.write_log();
            }
        }
    }

    /// Append the last install's log lines to `tui.log`.
    fn write_log(&mut self) {
        let (Some(path), Some(entry)) = (&self.log_file, self.log.last()) else {
            return;
        };
        if let Err(error) = log::append(path, entry) {
            self.note = Some(format!("tui.log could not be written: {error}"));
        }
    }

    /// Start the dry run of installing `name` `version`.
    fn plan(&mut self, name: &str, version: &str) {
        if let Err(error) = &self.destination {
            self.note = Some(error.lines().next().unwrap_or_default().into());
            return;
        }
        if let Some(why) = self.busy() {
            self.note = Some(why);
            return;
        }
        let package = format!("{name}@{version}");
        let cancel = CancelToken::default();
        // The plan shows a downgrade as it is; only a confirmed run may make
        // one.
        self.start(Job::Install {
            package: package.clone(),
            dry_run: true,
            allow_downgrade: true,
            cancel: cancel.clone(),
        });
        self.op = Some(Op {
            package,
            preview: Preview::default(),
            stage: Stage::Planning,
            cancel,
        });
    }

    /// Run the install the preview showed.
    fn run_install(&mut self, allow_downgrade: bool) {
        let Some(op) = &mut self.op else {
            return;
        };
        op.cancel = CancelToken::default();
        op.stage = Stage::Running(Progress::default());
        let job = Job::Install {
            package: op.package.clone(),
            dry_run: false,
            allow_downgrade,
            cancel: op.cancel.clone(),
        };
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        let header = log::header(seconds, &format!("install {}", op.package));
        self.log.push(vec![header]);
        self.start(job);
    }

    /// The list's `i`: plan the selected package's newest version.
    fn install_selected(&mut self) {
        let Listing::Loaded(browse) = &self.listing else {
            return;
        };
        let Some(package) = browse.selected() else {
            return;
        };
        let name = package.name.clone();
        match (&package.status, &package.available) {
            (Status::NeedsLabview(year), _) => {
                self.note = Some(format!("{name} needs LabVIEW {year}"));
            }
            (_, None) => self.note = Some(format!("no source has {name}")),
            _ if resolvable(&package.versions, self.target()).is_none() => {
                self.note = Some(format!("no version of {name} fits this LabVIEW"));
            }
            (_, Some(version)) => {
                let version = version.to_string();
                self.plan(&name, &version);
            }
        }
    }

    /// A key while an install is on screen. Once the install runs, only
    /// cancel and the log answer until it ends.
    fn op_key(&mut self, key: KeyEvent) {
        let Some(op) = &mut self.op else {
            return;
        };
        let quit = keys::QUIT.matches(key);
        match &mut op.stage {
            Stage::Planning | Stage::PlanFailed(_) | Stage::Preview if keys::ABORT.matches(key) => {
                op.cancel.cancel();
                self.op = None;
            }
            Stage::Planning | Stage::PlanFailed(_) | Stage::Preview | Stage::ConfirmDowngrade
                if quit =>
            {
                op.cancel.cancel();
                self.running = false;
            }
            Stage::Preview if keys::CONFIRM.matches(key) => {
                match op.preview.downgrade().is_some() {
                    true => op.stage = Stage::ConfirmDowngrade,
                    false => self.run_install(false),
                }
            }
            Stage::ConfirmDowngrade if keys::YES.matches(key) => self.run_install(true),
            Stage::ConfirmDowngrade if keys::NO.matches(key) => op.stage = Stage::Preview,
            Stage::Running(_) if keys::LOG.matches(key) => self.showing_log = true,
            Stage::Running(progress) if progress.outcome.is_none() => {
                if keys::CANCEL.matches(key) {
                    // The token refuses once files are being written.
                    match progress.cancellable() && op.cancel.cancel() {
                        true => progress.cancelling = true,
                        false => progress.refused = true,
                    }
                } else {
                    let name = match key.code {
                        KeyCode::Char(c) => c.to_string(),
                        other => format!("{other:?}"),
                    };
                    self.note = Some(format!("{name} waits until the operation ends"));
                }
            }
            Stage::Running(_) if keys::CLOSE.matches(key) => {
                self.op = None;
                // What the install changed shows in the list.
                self.start(Job::Load { refresh: false });
            }
            Stage::Running(_) if quit => self.running = false,
            _ => {}
        }
    }

    /// Show a catalog read. A reload that fails keeps the list it would
    /// have replaced, marked with why.
    fn listed(&mut self, listing: Result<(Catalog, Vec<Manifest>), String>) {
        let target = self.target().cloned();
        match (listing, &mut self.listing) {
            (Ok((catalog, installed)), old) => {
                let mut browse = Browse::new(catalog, &installed, target.as_ref());
                if let Listing::Loaded(old) = old {
                    browse.narrow_as(old);
                }
                // An open view shows the package as listed now, or closes
                // if the feeds no longer list it.
                self.detail = self.detail.take().and_then(|detail| {
                    let package = browse
                        .packages
                        .iter()
                        .find(|p| same_name(&p.name, &detail.name));
                    package.map(|package| detail.reopened(package))
                });
                self.listing = Listing::Loaded(browse);
                // The feeds may list other details now.
                self.details.clear();
            }
            (Err(error), Listing::Loaded(old)) => old.stale = Some(error),
            (Err(error), _) => self.listing = Listing::Failed(error),
        }
    }

    /// The worker ended without a result: it panicked.
    fn lost(&mut self) {
        let Some((job, _)) = self.pending.take() else {
            return;
        };
        let why = "stopped without a result";
        match job {
            Job::Load { .. } => self.listed(Err(format!("reading the catalog {why}"))),
            Job::Info { name, version } => {
                let error = format!("reading the details of {name} {version} {why}");
                self.details.insert((name, version), Err(error));
            }
            Job::Install {
                package, dry_run, ..
            } => {
                let error = format!("installing {package} {why}");
                self.installed(dry_run, Outcome::Failed(error));
            }
        }
        self.fetch_wanted();
    }

    /// Take what the running job has sent: its events and its result.
    pub fn poll(&mut self) {
        while let Some((_, pending)) = &self.pending {
            match pending.try_recv() {
                Ok(done) => self.finish(done),
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => self.lost(),
            }
        }
    }

    /// Block until no job runs: the one running, and any its result starts.
    pub fn wait_for_job(&mut self) {
        while let Some((_, pending)) = &self.pending {
            match pending.recv() {
                Ok(done) => self.finish(done),
                Err(_) => self.lost(),
            }
        }
    }

    /// Act on one terminal event. Only key presses count: Windows also
    /// reports each key's release. A resize needs nothing here, as each draw
    /// takes the terminal's size as it is then.
    pub fn handle(&mut self, event: Event) {
        let Event::Key(key) = event else {
            return;
        };
        if key.kind != KeyEventKind::Press {
            return;
        }
        self.note = None;
        if self.showing_log {
            if keys::LOG.matches(key) || keys::BACK.matches(key) {
                self.showing_log = false;
            } else if self.installing() && keys::CANCEL.matches(key) {
                self.showing_log = false;
                self.op_key(key);
            } else if keys::INTERRUPT.matches(key) && !self.installing() {
                self.running = false;
            }
            return;
        }
        if self.op.is_some() {
            self.op_key(key);
            return;
        }
        if let Listing::Loaded(browse) = &mut self.listing
            && browse.searching
        {
            if keys::INTERRUPT.matches(key) {
                self.running = false;
            } else {
                search_key(browse, key);
            }
            return;
        }
        if keys::QUIT.matches(key) {
            self.running = false;
        } else if let Some(detail) = &mut self.detail {
            if keys::BACK.matches(key) {
                self.detail = None;
            } else if keys::INSTALL.matches(key) {
                let (name, version) = detail.key();
                let entry = detail.entry().clone();
                // An exact pin skips core's LabVIEW check, so the TUI refuses it here.
                match self.target().and_then(|t| needs_newer_labview(&entry, t)) {
                    Some(year) => {
                        self.note = Some(format!("{name} {version} needs LabVIEW {year}"));
                    }
                    None => self.plan(&name, &version),
                }
            } else {
                detail_key(detail, key);
            }
        } else if keys::LOG.matches(key) {
            self.showing_log = true;
        } else if keys::RELOAD.matches(key) {
            match self.busy() {
                Some(why) => self.note = Some(why),
                None => self.start(Job::Load { refresh: true }),
            }
        } else if let Listing::Loaded(browse) = &mut self.listing {
            if keys::INSTALL.matches(key) {
                self.install_selected();
            } else if keys::OPEN.matches(key) {
                self.detail = browse.selected().map(Detail::new);
            } else {
                browse_key(browse, key);
            }
        }
        self.fetch_wanted();
    }
}

/// A key while the search box has the keys: Enter keeps the search, Esc
/// drops it.
fn search_key(browse: &mut Browse, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => browse.clear_search(),
        KeyCode::Enter => browse.searching = false,
        KeyCode::Backspace => browse.backspace(),
        KeyCode::Char(c) if TYPING.contains(&key.modifiers) => browse.type_char(c),
        _ => {}
    }
}

/// The moves Up, Down, PgUp, PgDn, Home and End make, `page` rows a page.
fn moves(key: KeyEvent, page: isize) -> Option<isize> {
    let table: [(&keys::Binding, isize); 6] = [
        (&keys::UP, -1),
        (&keys::DOWN, 1),
        (&keys::PAGE_UP, -page),
        (&keys::PAGE_DOWN, page),
        (&keys::HOME, isize::MIN),
        (&keys::END, isize::MAX),
    ];
    let found = table.iter().find(|(binding, _)| binding.matches(key));
    found.map(|&(_, by)| by)
}

/// A key that moves, searches or filters the list.
fn browse_key(browse: &mut Browse, key: KeyEvent) {
    if let Some(by) = moves(key, browse.page_size()) {
        browse.step(by);
    } else if keys::SEARCH.matches(key) {
        browse.searching = true;
    } else if keys::FILTER.matches(key) {
        browse.next_filter();
    }
}

/// A key on the detail view: Tab swaps which pane Up and Down move; PgUp and
/// PgDn always scroll the fields.
fn detail_key(detail: &mut Detail, key: KeyEvent) {
    let page = detail.page.max(1) as isize;
    if keys::FOCUS.matches(key) {
        detail.focus = match detail.focus {
            Focus::Versions => Focus::Fields,
            Focus::Fields => Focus::Versions,
        };
    } else if let Some(by) = moves(key, page) {
        let paged = keys::PAGE_UP.matches(key) || keys::PAGE_DOWN.matches(key);
        match (detail.focus, paged) {
            (Focus::Versions, false) => detail.step(by),
            _ => detail.scroll_by(by),
        }
    }
}

/// The modifiers a typed character can come with. On Windows, AltGr (which
/// types `@` or `\` on some layouts) may come as Ctrl+Alt, with or without
/// Shift.
const TYPING: [KeyModifiers; 4] = [
    KeyModifiers::NONE,
    KeyModifiers::SHIFT,
    KeyModifiers::CONTROL.union(KeyModifiers::ALT),
    KeyModifiers::CONTROL
        .union(KeyModifiers::ALT)
        .union(KeyModifiers::SHIFT),
];

/// Take the worker's result, draw, then handle the next event if one came,
/// until the app stops running.
pub fn run<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    mut next_event: impl FnMut() -> Result<Option<Event>, B::Error>,
) -> Result<(), B::Error> {
    loop {
        app.poll();
        terminal.draw(|frame| ui::draw(frame, app))?;
        if !app.is_running() {
            return Ok(());
        }
        if let Some(event) = next_event()? {
            app.handle(event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An app with nothing loading, so no test reads a feed.
    fn idle() -> App {
        let builder = EnvBuilder::new(
            lvpm_core::config::Config {
                cache: "cache".into(),
                local_sources: Vec::new(),
                venv_dir: ".venv".into(),
            },
            Vec::new(),
            ".".into(),
        );
        App {
            running: true,
            destination: builder.destination(),
            builder,
            listing: Listing::Loading,
            detail: None,
            details: Details::new(),
            note: None,
            pending: None,
            op: None,
            log: Vec::new(),
            showing_log: false,
            log_file: None,
            feeds: Vec::new(),
        }
    }

    fn after(code: KeyCode, modifiers: KeyModifiers, kind: KeyEventKind) -> bool {
        let mut app = idle();
        app.handle(Event::Key(KeyEvent::new_with_kind(code, modifiers, kind)));
        app.is_running()
    }

    #[test]
    fn ctrl_c_quits() {
        assert!(!after(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
            KeyEventKind::Press
        ));
    }

    #[test]
    fn q_with_a_modifier_and_a_repeated_q_do_not_quit() {
        assert!(after(
            KeyCode::Char('q'),
            KeyModifiers::CONTROL,
            KeyEventKind::Press
        ));
        assert!(after(
            KeyCode::Char('Q'),
            KeyModifiers::SHIFT,
            KeyEventKind::Press
        ));
        assert!(after(
            KeyCode::Char('q'),
            KeyModifiers::NONE,
            KeyEventKind::Repeat
        ));
    }

    fn press(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
        app.handle(Event::Key(KeyEvent::new(code, modifiers)));
    }

    /// An idle app showing `alpha`, which fits, and `gamma`, which needs
    /// LabVIEW 2099.
    fn listing() -> App {
        let entry = |name: &str, lv_min: f64| lvpm_core::index::Entry {
            name: name.into(),
            version: lvpm_core::version::Version::parse("1.0"),
            url: String::new(),
            md5: None,
            display_name: None,
            requires: Vec::new(),
            lv_min: Some(lv_min),
            source: String::new(),
        };
        let catalog = lvpm_core::ops::query::Catalog {
            entries: vec![entry("alpha", 20.0), entry("gamma", 99.0)],
            sources: Vec::new(),
            failed: Vec::new(),
        };
        let labview_2025 = LvTarget {
            version: 25.0,
            bitness: 64,
            path: "LabVIEW 2025".into(),
        };
        let mut app = idle();
        app.listing = Listing::Loaded(Browse::new(catalog, &[], Some(&labview_2025)));
        app
    }

    fn browse(app: &App) -> &Browse {
        match &app.listing {
            Listing::Loaded(browse) => browse,
            other => panic!("not loaded: {other:?}"),
        }
    }

    #[test]
    fn the_search_box_takes_q_as_a_letter_and_still_quits_on_ctrl_c() {
        let mut app = listing();
        press(&mut app, KeyCode::Char('/'), KeyModifiers::SHIFT);
        press(&mut app, KeyCode::Char('q'), KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('x'), KeyModifiers::CONTROL);
        press(
            &mut app,
            KeyCode::Char('@'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        );
        assert!(app.is_running());
        assert_eq!(
            browse(&app).search,
            "q@",
            "Ctrl-x types nothing, AltGr does"
        );
        press(&mut app, KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(browse(&app).search, "q");
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(!browse(&app).searching);
        assert_eq!(browse(&app).search, "q", "Enter keeps the search");

        press(&mut app, KeyCode::Char('/'), KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(!app.is_running());
    }

    #[test]
    fn a_note_says_why_install_cannot_start_until_the_next_key() {
        let mut app = listing();
        press(&mut app, KeyCode::Char('i'), KeyModifiers::NONE);
        let nowhere = "install needs a LabVIEW or --prefix";
        assert_eq!(app.note.as_deref(), Some(nowhere), "alpha fits, but where");
        assert!(app.op.is_none());
        press(&mut app, KeyCode::Down, KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('i'), KeyModifiers::NONE);
        assert_eq!(app.note.as_deref(), Some("gamma needs LabVIEW 2099"));
        press(&mut app, KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(app.note, None);
    }

    #[test]
    fn reload_while_a_job_runs_starts_no_second_one() {
        let mut app = idle();
        let (send, receive) = std::sync::mpsc::channel();
        app.pending = Some((Job::Load { refresh: false }, receive));
        press(&mut app, KeyCode::Char('R'), KeyModifiers::SHIFT);
        assert_eq!(app.note.as_deref(), Some("the catalog is still loading"));
        // The first job is still the one the app waits for.
        send.send(Done::Loaded {
            destination: Err("none".into()),
            listing: Err("first job".into()),
        })
        .unwrap();
        app.poll();
        assert!(
            matches!(&app.listing, Listing::Failed(e) if e == "first job"),
            "{:?}",
            app.listing
        );
    }

    #[test]
    fn a_worker_that_ends_without_a_result_fails_the_listing() {
        let mut app = idle();
        let (send, receive) = std::sync::mpsc::channel::<Done>();
        app.pending = Some((Job::Load { refresh: false }, receive));
        app.poll();
        assert!(matches!(app.listing, Listing::Loading));
        drop(send);
        app.poll();
        assert!(
            matches!(app.listing, Listing::Failed(_)),
            "{:?}",
            app.listing
        );
        assert!(app.pending.is_none());
    }

    #[test]
    fn the_loop_shows_the_result_that_arrives_between_keys() {
        let mut app = idle();
        let (send, receive) = std::sync::mpsc::channel();
        app.pending = Some((Job::Load { refresh: false }, receive));
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        let mut ticks = 0;
        run(&mut terminal, &mut app, || {
            ticks += 1;
            Ok(match ticks {
                1 => {
                    send.send(Done::Loaded {
                        destination: Err("none".into()),
                        listing: Err("no source could be read".into()),
                    })
                    .unwrap();
                    None
                }
                2 => None,
                _ => Some(Event::Key(KeyEvent::new(
                    KeyCode::Char('q'),
                    KeyModifiers::NONE,
                ))),
            })
        })
        .unwrap();
        let screen = format!("{:?}", terminal.backend().buffer());
        assert!(screen.contains("no source could be read"), "{screen}");
    }

    #[test]
    fn a_long_note_shows_whole_on_the_footer() {
        let mut app = listing();
        let note = "jki_lib_state_machine_extended needs LabVIEW 2099";
        app.note = Some(note.into());
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let footer: String = (0..80).map(|x| buffer[(x, 23)].symbol()).collect();
        assert!(footer.starts_with(note), "{footer}");
    }

    fn catalog(names: &[&str]) -> Catalog {
        let entry = |name: &str| lvpm_core::index::Entry {
            name: name.into(),
            version: lvpm_core::version::Version::parse("1.0"),
            url: String::new(),
            md5: None,
            display_name: None,
            requires: Vec::new(),
            lv_min: Some(20.0),
            source: String::new(),
        };
        Catalog {
            entries: names.iter().map(|n| entry(n)).collect(),
            sources: Vec::new(),
            failed: Vec::new(),
        }
    }

    /// Hand `done` to `app` as the result of `job`.
    fn complete(app: &mut App, job: Job, done: Done) {
        let (send, receive) = std::sync::mpsc::channel();
        app.pending = Some((job, receive));
        send.send(done).unwrap();
        app.poll();
    }

    fn reloaded(listing: Result<(Catalog, Vec<Manifest>), String>) -> Done {
        Done::Loaded {
            destination: Err("none".into()),
            listing,
        }
    }

    #[test]
    fn a_reload_keeps_the_search_and_the_selected_package_and_a_failed_one_the_list() {
        let mut app = listing();
        press(&mut app, KeyCode::Down, KeyModifiers::NONE);
        let reload = Job::Load { refresh: true };
        let fresh = catalog(&["alpha", "beta", "gamma"]);
        complete(&mut app, reload.clone(), reloaded(Ok((fresh, Vec::new()))));
        let shown = browse(&app).selected().map(|p| p.name.as_str());
        assert_eq!(shown, Some("gamma"), "the selection follows the name");
        assert_eq!(browse(&app).packages.len(), 3);

        complete(&mut app, reload, reloaded(Err("feed down".into())));
        assert_eq!(browse(&app).stale.as_deref(), Some("feed down"));
        assert_eq!(browse(&app).packages.len(), 3, "the old list stays");
    }

    #[test]
    fn the_detail_view_moves_the_focused_pane_and_esc_returns_to_the_list() {
        let mut app = listing();
        // A job that never ends, so opening the view reads nothing.
        let (_send, receive) = std::sync::mpsc::channel();
        app.pending = Some((Job::Load { refresh: false }, receive));
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        let detail = app.detail.as_ref().expect("Enter opens the details");
        assert_eq!(detail.name, "alpha");
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE);
        let detail = app.detail.as_ref().unwrap();
        assert_eq!((detail.focus, detail.scroll), (Focus::Fields, 1));
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.detail.is_none());
        assert_eq!(browse(&app).selected().unwrap().name, "alpha");
    }

    #[test]
    fn install_from_the_details_refuses_a_version_that_needs_a_newer_labview() {
        let mut app = listing();
        let labview_2025 = LvTarget {
            version: 25.0,
            bitness: 64,
            path: "LabVIEW 2025".into(),
        };
        app.destination = Ok(Destination::Global(lvpm_core::target::Roots::labview(
            &labview_2025,
        )));
        press(&mut app, KeyCode::Down, KeyModifiers::NONE);
        app.detail = browse(&app).selected().map(Detail::new);
        // A job that never ends, so the view reads nothing.
        let (_send, receive) = std::sync::mpsc::channel();
        app.pending = Some((Job::Load { refresh: false }, receive));
        press(&mut app, KeyCode::Char('i'), KeyModifiers::NONE);
        assert_eq!(app.note.as_deref(), Some("gamma 1.0 needs LabVIEW 2099"));
        assert!(app.op.is_none());
    }

    #[test]
    fn a_details_job_that_ends_without_a_result_is_shown_as_failed() {
        let mut app = listing();
        app.detail = browse(&app).selected().map(Detail::new);
        let info = Job::Info {
            name: "alpha".into(),
            version: "1.0".into(),
        };
        let (send, receive) = std::sync::mpsc::channel::<Done>();
        app.pending = Some((info, receive));
        drop(send);
        app.poll();
        let key = ("alpha".to_string(), "1.0".to_string());
        let error = app.details[&key].as_ref().unwrap_err();
        assert!(error.contains("stopped without a result"), "{error}");
        assert!(app.pending.is_none(), "a read detail is not read again");
    }

    #[test]
    fn a_reload_keeps_the_search_and_the_filter_and_drops_a_selection_no_longer_listed() {
        let mut app = listing();
        for c in ['/', 'a', 'l'] {
            press(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        let reload = Job::Load { refresh: true };
        let fresh = catalog(&["alps", "beta", "gamma"]);
        complete(&mut app, reload.clone(), reloaded(Ok((fresh, Vec::new()))));
        let shown = browse(&app);
        assert_eq!(shown.search, "al");
        let rows: Vec<&str> = shown
            .rows
            .iter()
            .map(|&i| shown.packages[i].name.as_str())
            .collect();
        assert_eq!(rows, ["alps"], "the search narrows the new list");
        assert_eq!(shown.selected().unwrap().name, "alps", "alpha is gone");

        press(&mut app, KeyCode::Char('f'), KeyModifiers::NONE);
        let filter = browse(&app).filter;
        complete(
            &mut app,
            reload,
            reloaded(Ok((catalog(&["alps"]), Vec::new()))),
        );
        assert_eq!(browse(&app).filter, filter);
    }

    #[test]
    fn a_reload_reopens_the_detail_view_on_the_new_listing_or_closes_it() {
        let mut app = listing();
        app.detail = browse(&app).selected().map(Detail::new);
        let key = ("alpha".to_string(), "1.0".to_string());
        app.details
            .insert(key, Err("read before the reload".into()));
        let mut fresh = catalog(&["alpha"]);
        let mut newer = fresh.entries[0].clone();
        newer.version = lvpm_core::version::Version::parse("2.0");
        fresh.entries.insert(0, newer);
        // The list itself, as the finished job's follow-up read would reach
        // past this test's feeds.
        app.listed(Ok((fresh, Vec::new())));
        let detail = app.detail.as_ref().expect("alpha is still listed");
        let versions: Vec<String> = detail
            .versions
            .iter()
            .map(|e| e.version.to_string())
            .collect();
        assert_eq!(versions, ["2.0", "1.0"]);
        assert_eq!(detail.entry().version.to_string(), "1.0", "the pick stays");
        assert!(app.details.is_empty(), "the details are read again");

        app.listed(Ok((catalog(&["beta"]), Vec::new())));
        assert!(app.detail.is_none(), "alpha is no longer listed");
    }

    #[test]
    fn the_header_says_reloading_only_while_a_list_is_shown() {
        let mut app = idle();
        let (_send, receive) = std::sync::mpsc::channel();
        app.pending = Some((Job::Load { refresh: false }, receive));
        assert!(!app.reloading(), "the first load");
        let (_send, receive) = std::sync::mpsc::channel();
        let mut app = listing();
        app.pending = Some((Job::Load { refresh: true }, receive));
        assert!(app.reloading());
        press(&mut app, KeyCode::Char('R'), KeyModifiers::SHIFT);
        assert_eq!(app.note.as_deref(), Some("the catalog is still loading"));
    }

    #[test]
    fn the_load_shows_each_feed_as_it_is_asked_and_answered() {
        use lvpm_core::events::{Event, FeedOutcome};
        let mut app = idle();
        let (send, receive) = std::sync::mpsc::channel();
        app.pending = Some((Job::Load { refresh: false }, receive));
        let (a, b) = ("http://a/index.vipr", "http://b/index.vipr");
        for event in [
            Event::FeedCheckStart {
                url: a.into(),
                first_run: true,
            },
            Event::FeedChecked {
                url: a.into(),
                outcome: FeedOutcome::Downloaded,
                bytes: 3 * 1_048_576,
                elapsed: std::time::Duration::from_millis(4200),
            },
            Event::FeedCheckStart {
                url: b.into(),
                first_run: false,
            },
        ] {
            send.send(Done::Event(event)).expect("the app receives");
        }
        app.poll();
        let feeds = [(a, "3.0 MB in 4.2s"), (b, "checking…")];
        let shown: Vec<_> = app
            .feeds
            .iter()
            .map(|(u, s)| (u.as_str(), s.as_str()))
            .collect();
        assert_eq!(shown, feeds);
        assert!(app.op.is_none(), "a load's events start no install");
    }

    #[test]
    fn reload_while_details_are_read_waits_for_them() {
        let mut app = listing();
        let (_send, receive) = std::sync::mpsc::channel();
        let info = Job::Info {
            name: "alpha".into(),
            version: "1.0".into(),
        };
        app.pending = Some((info, receive));
        press(&mut app, KeyCode::Char('R'), KeyModifiers::SHIFT);
        assert_eq!(app.note.as_deref(), Some("the details are still loading"));
        assert!(matches!(app.pending, Some((Job::Info { .. }, _))));
    }
}
