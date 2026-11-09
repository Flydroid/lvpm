//! lvpm's log: one continuous file per venv, `.lv-venv/logs/lvpm.log`.
//!
//! Every run appends to it: a header naming the command, everything the run
//! printed (as `info`), detail that only goes to the file (`debug`: each file
//! linkfix touched, each download), and how the run ended. Records written
//! before the venv is known (argument checks, the lookup itself) are held and
//! written once it is; a run that never finds a venv writes to the user's
//! global log instead (`%LOCALAPPDATA%\lvpm\logs\lvpm.log` on Windows).
//!
//! The file level is `--log-level`, else `LVPM_LOG`, else `debug`. The console
//! is unchanged by it: what a command prints, it prints at every level.
//!
//! The file is opened in append mode and written one record per write, with
//! no buffer, so a run cut off by Ctrl+C or a crash keeps what it logged and
//! two lvpm processes on one venv interleave whole lines. Logging never fails
//! a command: a log that cannot be written is reported once and dropped.

use log::{Level, LevelFilter, Log, Metadata, Record};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Name of the log file inside a logs folder.
pub const FILE_NAME: &str = "lvpm.log";
/// Size at which the log is moved aside to `lvpm.log.1` (replacing the older one).
const ROTATE_AT: u64 = 10 * 1024 * 1024;

struct State {
    file: Option<File>,
    path: Option<PathBuf>,
    /// Lines formatted before a file was attached.
    pending: Vec<String>,
    /// Text printed with `logp!`/`elogp!` that has no line end yet, per stream.
    partial: [String; 2],
    failed: bool,
}

static STATE: Mutex<State> = Mutex::new(State {
    file: None,
    path: None,
    pending: Vec::new(),
    partial: [String::new(), String::new()],
    failed: false,
});

struct Logger;

impl Log for Logger {
    fn enabled(&self, m: &Metadata) -> bool {
        // Other crates (reqwest, hyper) log their internals at debug; only
        // their warnings and errors belong in lvpm's log.
        let ours = m.target() == "lvpm" || m.target().starts_with("lvpm::");
        m.level() <= log::max_level() && (ours || m.level() <= Level::Warn)
    }

    fn log(&self, r: &Record) {
        if self.enabled(r.metadata()) {
            write(r.level(), r.target(), &r.args().to_string());
        }
    }

    fn flush(&self) {}
}

/// The level `--log-level` or `LVPM_LOG` names; `debug` when neither does.
pub fn level(arg: Option<&str>) -> Result<LevelFilter, String> {
    let env = std::env::var("LVPM_LOG").ok();
    match arg.or(env.as_deref()) {
        None => Ok(LevelFilter::Debug),
        Some(s) => s
            .parse()
            .map_err(|_| format!("unknown log level {s:?}: use off, error, warn, info, debug or trace")),
    }
}

/// Install the logger and write the run's header line.
pub fn init(level: LevelFilter) {
    if log::set_logger(&Logger).is_ok() {
        log::set_max_level(level);
    }
    let args: Vec<String> = std::env::args().skip(1).map(|a| quote(&a)).collect();
    let cwd = std::env::current_dir().map(|d| d.display().to_string()).unwrap_or_default();
    log::info!(target: "lvpm", "---- lvpm {} {} (in {cwd})", env!("CARGO_PKG_VERSION"), args.join(" "));
}

fn quote(a: &str) -> String {
    if a.is_empty() || a.contains(char::is_whitespace) { format!("\"{a}\"") } else { a.to_string() }
}

/// Write the log into `dir` (created if missing) from now on, starting with
/// whatever was logged before. A second call for the same folder does nothing.
pub fn attach(dir: &Path) {
    let path = dir.join(FILE_NAME);
    let mut s = lock();
    if s.path.as_deref() == Some(path.as_path()) {
        return;
    }
    let opened = std::fs::create_dir_all(dir).and_then(|_| {
        if std::fs::metadata(&path).is_ok_and(|m| m.len() >= ROTATE_AT) {
            let _ = std::fs::rename(&path, dir.join(format!("{FILE_NAME}.1")));
        }
        OpenOptions::new().create(true).append(true).open(&path)
    });
    match opened {
        Ok(mut f) => {
            for line in std::mem::take(&mut s.pending) {
                let _ = f.write_all(line.as_bytes());
            }
            s.file = Some(f);
            s.path = Some(path);
        }
        Err(e) => report_failure(&mut s, &path, &e),
    }
}

/// Where the log of this run went, once a file is attached.
pub fn path() -> Option<PathBuf> {
    lock().path.clone()
}

/// Close the run: log how it ended, and write to `fallback` if no venv took
/// the log. Unfinished `logp!` text is written as a line of its own.
pub fn finish(result: &anyhow::Result<()>, fallback: Option<PathBuf>) {
    for err in [false, true] {
        let rest = std::mem::take(&mut lock().partial[err as usize]);
        if !rest.is_empty() {
            write(Level::Info, "lvpm", &rest);
        }
    }
    match result {
        Ok(()) => log::info!(target: "lvpm", "---- done"),
        Err(e) => log::error!(target: "lvpm", "---- failed: {e:#}"),
    }
    let attached = lock().file.is_some();
    if !attached && let Some(dir) = fallback {
        attach(&dir);
    }
}

fn lock() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|p| p.into_inner())
}

fn report_failure(s: &mut State, path: &Path, e: &std::io::Error) {
    if !s.failed {
        s.failed = true;
        eprintln!("note: cannot write the lvpm log {}: {e}", path.display());
    }
}

/// Format one record, a line per line of its message, and write it out.
fn write(level: Level, target: &str, msg: &str) {
    let target = target.strip_prefix("lvpm::").unwrap_or(target);
    let head = format!("{} {:>6} {:<5} {target}:", timestamp(), std::process::id(), level);
    // Blank lines space the console out; in the log they are noise.
    let mut text = String::new();
    for line in msg.lines().filter(|l| !l.trim().is_empty()) {
        text.push_str(&head);
        text.push(' ');
        text.push_str(line);
        text.push('\n');
    }
    if text.is_empty() {
        return;
    }
    let mut s = lock();
    let written = s.file.as_mut().map(|f| f.write_all(text.as_bytes()));
    match written {
        None => s.pending.push(text),
        Some(Ok(())) => {}
        Some(Err(e)) => {
            let path = s.path.clone().unwrap_or_default();
            report_failure(&mut s, &path, &e);
            s.file = None;
        }
    }
}

/// Print `text` and a line end to stdout (`err` false) or stderr, and log the
/// completed line with any `part` text printed before it.
pub fn line(target: &str, level: Level, err: bool, text: String) {
    if err {
        eprintln!("{text}");
    } else {
        println!("{text}");
    }
    let before = std::mem::take(&mut lock().partial[err as usize]);
    if level <= log::max_level() {
        write(level, target, &(before + &text));
    }
}

/// Print `text` without a line end. It is logged once its line is complete.
pub fn part(target: &str, err: bool, text: String) {
    if err {
        eprint!("{text}");
        let _ = std::io::stderr().flush();
    } else {
        print!("{text}");
        let _ = std::io::stdout().flush();
    }
    let done: Vec<String> = {
        let mut s = lock();
        let buf = &mut s.partial[err as usize];
        buf.push_str(&text);
        match buf.rfind('\n') {
            Some(i) => {
                let rest = buf.split_off(i + 1);
                let full = std::mem::replace(buf, rest);
                full.lines().map(str::to_string).collect()
            }
            None => Vec::new(),
        }
    };
    if Level::Info <= log::max_level() {
        for l in done {
            write(Level::Info, target, &l);
        }
    }
}

/// `2026-11-09T18:52:35.123Z`: UTC, so lines from different machines and
/// daylight-saving changes sort the same.
fn timestamp() -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = now.as_secs();
    let (h, m, s) = ((secs / 3600) % 24, (secs / 60) % 60, secs % 60);
    let (y, mo, d) = civil(secs / 86_400);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}.{:03}Z", now.subsec_millis())
}

/// Days since 1970-01-01 as a proleptic Gregorian date (Howard Hinnant's algorithm).
fn civil(days: u64) -> (i64, u32, u32) {
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(mo <= 2);
    (y, mo, d)
}

// What a command shows the user, printed and logged in one call. Unlike the
// `log` crate's macros (`log::debug!` & co., file only), these always print:
//
//   log!   println + info      elog!   eprintln + info
//   wlog!  println + warn      ewlog!  eprintln + warn
//   logp!  print, logged once its line is complete; elogp! the same on stderr
//
// The crate's own macros are always called by path (`log::debug!`), so this
// `log!` never stands in for `log::log!`.

/// `println!` that also logs the line (`info`).
macro_rules! log {
    () => { $crate::logging::line(module_path!(), log::Level::Info, false, String::new()) };
    ($($t:tt)*) => { $crate::logging::line(module_path!(), log::Level::Info, false, format!($($t)*)) };
}

/// `eprintln!` that also logs the line (`info`).
macro_rules! elog {
    () => { $crate::logging::line(module_path!(), log::Level::Info, true, String::new()) };
    ($($t:tt)*) => { $crate::logging::line(module_path!(), log::Level::Info, true, format!($($t)*)) };
}

/// `println!` of a failure or warning: logged at `warn`.
macro_rules! wlog {
    ($($t:tt)*) => { $crate::logging::line(module_path!(), log::Level::Warn, false, format!($($t)*)) };
}

/// `eprintln!` of a failure or warning: logged at `warn`.
macro_rules! ewlog {
    ($($t:tt)*) => { $crate::logging::line(module_path!(), log::Level::Warn, true, format!($($t)*)) };
}

/// `print!` that logs the text once its line is complete.
macro_rules! logp {
    ($($t:tt)*) => { $crate::logging::part(module_path!(), false, format!($($t)*)) };
}

/// `eprint!` that logs the text once its line is complete.
macro_rules! elogp {
    ($($t:tt)*) => { $crate::logging::part(module_path!(), true, format!($($t)*)) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_convert_across_leap_years() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(11_016), (2000, 2, 29));
        assert_eq!(civil(20_766), (2026, 11, 9));
    }

    #[test]
    fn levels_parse_and_default_to_debug() {
        assert_eq!(level(Some("warn")), Ok(LevelFilter::Warn));
        assert_eq!(level(Some("TRACE")), Ok(LevelFilter::Trace));
        assert!(level(Some("loud")).is_err());
    }
}
