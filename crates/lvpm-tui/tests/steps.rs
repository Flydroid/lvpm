//! Steps that press keys and read the screen the TUI drew.

use crate::TuiWorld;
use cucumber::{given, then, when};
use lvpm_tui::{app, ui};
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

impl TuiWorld {
    /// The screen the TUI draws now, one string per row.
    pub fn screen(&mut self) -> Vec<String> {
        self.app();
        let (Some(app), terminal) = (&mut self.app, &mut self.terminal) else {
            unreachable!("the TUI started");
        };
        // As the event loop does before each draw.
        app.poll();
        terminal
            .draw(|frame| ui::draw(frame, app))
            .expect("drawing on a test terminal");
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect()
            })
            .collect()
    }

    pub fn press(&mut self, code: KeyCode, kind: KeyEventKind) {
        let event = KeyEvent::new_with_kind(code, KeyModifiers::NONE, kind);
        self.app().handle(Event::Key(event));
    }

    pub fn key(&mut self, key: &str, kind: KeyEventKind) {
        let (code, modifiers) = match key {
            "Esc" => (KeyCode::Esc, KeyModifiers::NONE),
            "Ctrl-C" => (KeyCode::Char('c'), KeyModifiers::CONTROL),
            _ => (KeyCode::Char(one_char(key)), KeyModifiers::NONE),
        };
        let event = KeyEvent::new_with_kind(code, modifiers, kind);
        self.app().handle(Event::Key(event));
    }
}

fn one_char(key: &str) -> char {
    let mut chars = key.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        panic!("one character keys only: {key:?}");
    };
    c
}

fn size(text: &str) -> (u16, u16) {
    let (width, height) = text.split_once('x').expect("a size such as 80x24");
    (
        width.parse().expect("a width"),
        height.parse().expect("a height"),
    )
}

#[given(regex = r"^the TUI runs at (\d+x\d+)$")]
async fn runs_at(world: &mut TuiWorld, at: String) {
    let (width, height) = size(&at);
    world.terminal.backend_mut().resize(width, height);
}

#[when(regex = r"^the terminal is resized to (\d+x\d+)$")]
async fn resized_to(world: &mut TuiWorld, to: String) {
    runs_at(world, to).await;
}

/// The header on the first row, the key footer on the last, and the body's
/// frame between them: its corners on the second and second-to-last rows.
#[then(regex = r"^the header bar and the key footer are drawn( with their borders)?$")]
async fn chrome_is_drawn(world: &mut TuiWorld) {
    let rows = world.screen();
    let (first, last) = (&rows[0], &rows[rows.len() - 1]);
    assert!(first.starts_with(" lvpm "), "header: {first:?}");
    assert!(last.contains("q quit"), "footer: {last:?}");
    let (top, bottom) = (&rows[1], &rows[rows.len() - 2]);
    assert!(
        top.starts_with('┌') && top.ends_with('┐'),
        "top border: {top:?}"
    );
    assert!(
        bottom.starts_with('└') && bottom.ends_with('┘'),
        "bottom border: {bottom:?}"
    );
}

#[then("it shows only a message asking for at least 80x24")]
async fn only_the_size_message(world: &mut TuiWorld) {
    let screen = world.screen().join("\n");
    assert!(screen.contains("at least 80x24"), "{screen}");
    assert!(!screen.contains("lvpm "), "the header is drawn:\n{screen}");
    assert!(!screen.contains('┌'), "a frame is drawn:\n{screen}");
}

#[when(expr = "{string} is pressed")]
async fn pressed(world: &mut TuiWorld, key: String) {
    world.key(&key, KeyEventKind::Press);
}

#[when(expr = "{string} is released without a press")]
async fn released(world: &mut TuiWorld, key: String) {
    world.key(&key, KeyEventKind::Release);
}

/// The real loop, handed an event source that must not be asked: a quit
/// TUI draws once more and returns.
#[then("the TUI's event loop ends without an error")]
async fn loop_ends(world: &mut TuiWorld) {
    world.app();
    let (Some(app), terminal) = (&mut world.app, &mut world.terminal) else {
        unreachable!("the TUI started");
    };
    let ended = app::run(terminal, app, || {
        panic!("the loop asked for another event after the TUI quit")
    });
    ended.expect("the loop ends");
    assert!(!app.is_running());
}

#[then("the TUI is still running")]
async fn still_running(world: &mut TuiWorld) {
    assert!(world.app().is_running());
}
