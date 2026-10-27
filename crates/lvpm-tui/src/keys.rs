//! The keys the TUI acts on. The footer and the key dispatch both read this
//! table, so a key is named and bound in one place.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// A key and what it does.
#[derive(Debug)]
pub struct Binding {
    /// How the footer names the key.
    pub key: &'static str,
    /// What the key does, in a word or two.
    pub action: &'static str,
    presses: &'static [(KeyCode, KeyModifiers)],
}

impl Binding {
    pub fn matches(&self, event: KeyEvent) -> bool {
        self.presses
            .iter()
            .any(|&(code, modifiers)| event.code == code && event.modifiers == modifiers)
    }
}

/// `q`, or Ctrl-C: in raw mode Ctrl-C is a key like any other.
pub const QUIT: Binding = Binding {
    key: "q",
    action: "quit",
    presses: &[
        (KeyCode::Char('q'), KeyModifiers::NONE),
        (KeyCode::Char('c'), KeyModifiers::CONTROL),
    ],
};

/// Ctrl-C alone: it still quits while the search box takes `q` as a letter.
pub const INTERRUPT: Binding = Binding {
    key: "Ctrl-C",
    action: "quit",
    presses: &[(KeyCode::Char('c'), KeyModifiers::CONTROL)],
};

/// `/`, which some layouts (German) may report with Shift.
pub const SEARCH: Binding = Binding {
    key: "/",
    action: "search",
    presses: &[
        (KeyCode::Char('/'), KeyModifiers::NONE),
        (KeyCode::Char('/'), KeyModifiers::SHIFT),
    ],
};

pub const FILTER: Binding = Binding {
    key: "f",
    action: "filter",
    presses: &[(KeyCode::Char('f'), KeyModifiers::NONE)],
};

/// Shift-r: terminals report the capital with Shift, some without.
pub const RELOAD: Binding = Binding {
    key: "R",
    action: "reload feeds",
    presses: &[
        (KeyCode::Char('R'), KeyModifiers::SHIFT),
        (KeyCode::Char('R'), KeyModifiers::NONE),
    ],
};

pub const OPEN: Binding = Binding {
    key: "Enter",
    action: "details",
    presses: &[(KeyCode::Enter, KeyModifiers::NONE)],
};

pub const BACK: Binding = Binding {
    key: "Esc",
    action: "back",
    presses: &[(KeyCode::Esc, KeyModifiers::NONE)],
};

/// Which of the detail view's panes Up and Down move.
pub const FOCUS: Binding = Binding {
    key: "Tab",
    action: "versions/fields",
    presses: &[(KeyCode::Tab, KeyModifiers::NONE)],
};

pub const INSTALL: Binding = Binding {
    key: "i",
    action: "install",
    presses: &[(KeyCode::Char('i'), KeyModifiers::NONE)],
};

/// Runs the install the preview shows.
pub const CONFIRM: Binding = Binding {
    key: "Enter",
    action: "confirm",
    presses: &[(KeyCode::Enter, KeyModifiers::NONE)],
};

/// Drops a plan, computed or being computed.
pub const ABORT: Binding = Binding {
    key: "Esc",
    action: "cancel",
    presses: &[(KeyCode::Esc, KeyModifiers::NONE)],
};

/// Runs a downgrade, asked a second time.
pub const YES: Binding = Binding {
    key: "y",
    action: "downgrade",
    presses: &[(KeyCode::Char('y'), KeyModifiers::NONE)],
};

pub const NO: Binding = Binding {
    key: "n",
    action: "back",
    presses: &[
        (KeyCode::Char('n'), KeyModifiers::NONE),
        (KeyCode::Esc, KeyModifiers::NONE),
    ],
};

/// Stops a running install; Esc and Ctrl-C do too, as nothing else may.
pub const CANCEL: Binding = Binding {
    key: "c",
    action: "cancel",
    presses: &[
        (KeyCode::Char('c'), KeyModifiers::NONE),
        (KeyCode::Esc, KeyModifiers::NONE),
        (KeyCode::Char('c'), KeyModifiers::CONTROL),
    ],
};

/// Closes an operation that ended.
pub const CLOSE: Binding = Binding {
    key: "Enter",
    action: "back to list",
    presses: &[
        (KeyCode::Enter, KeyModifiers::NONE),
        (KeyCode::Esc, KeyModifiers::NONE),
    ],
};

/// Shift-l, as reload's Shift-r.
pub const LOG: Binding = Binding {
    key: "L",
    action: "log",
    presses: &[
        (KeyCode::Char('L'), KeyModifiers::SHIFT),
        (KeyCode::Char('L'), KeyModifiers::NONE),
    ],
};

pub const UP: Binding = Binding {
    key: "↑",
    action: "up",
    presses: &[(KeyCode::Up, KeyModifiers::NONE)],
};

pub const DOWN: Binding = Binding {
    key: "↓",
    action: "down",
    presses: &[(KeyCode::Down, KeyModifiers::NONE)],
};

pub const PAGE_UP: Binding = Binding {
    key: "PgUp",
    action: "page up",
    presses: &[(KeyCode::PageUp, KeyModifiers::NONE)],
};

pub const PAGE_DOWN: Binding = Binding {
    key: "PgDn",
    action: "page down",
    presses: &[(KeyCode::PageDown, KeyModifiers::NONE)],
};

pub const HOME: Binding = Binding {
    key: "Home",
    action: "first",
    presses: &[(KeyCode::Home, KeyModifiers::NONE)],
};

pub const END: Binding = Binding {
    key: "End",
    action: "last",
    presses: &[(KeyCode::End, KeyModifiers::NONE)],
};

/// The keys the list's footer shows, in order. Enter is left out: with it,
/// why nothing can be installed no longer fits 80 columns.
pub const FOOTER: &[&Binding] = &[&SEARCH, &FILTER, &RELOAD, &QUIT];

/// The keys the detail view's footer shows, in order.
pub const DETAIL_FOOTER: &[&Binding] = &[&FOCUS, &PAGE_DOWN, &BACK, &QUIT];

pub const PLANNING_FOOTER: &[&Binding] = &[&ABORT, &QUIT];
pub const PLAN_FAILED_FOOTER: &[&Binding] = &[&BACK, &QUIT];
pub const PREVIEW_FOOTER: &[&Binding] = &[&CONFIRM, &ABORT];
pub const DOWNGRADE_FOOTER: &[&Binding] = &[&YES, &NO];
pub const RUNNING_FOOTER: &[&Binding] = &[&CANCEL, &LOG];
pub const RESULT_FOOTER: &[&Binding] = &[&CLOSE, &LOG, &QUIT];
pub const LOG_FOOTER: &[&Binding] = &[&BACK];
