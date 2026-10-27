//! Drawing: the header bar, the body and the key footer, or only a message
//! when the terminal is too small. Reverse video, not colour, marks the bars
//! and the selected row, so they read the same in conhost and with `NO_COLOR`.

use crate::app::{App, Details, Listing, Op, Stage};
use crate::browse::Browse;
use crate::destination;
use crate::detail::{self, Detail, Focus};
use crate::keys::{self, Binding};
use crate::plan::{Action, Preview};
use crate::progress::{self, PHASES, Progress, State};
use crate::worker::Outcome;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Row, Table, Wrap};

pub const MIN_WIDTH: u16 = 80;
pub const MIN_HEIGHT: u16 = 24;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        let message = format!(
            "The terminal is {}x{}; lvpm-tui needs at least {MIN_WIDTH}x{MIN_HEIGHT}.",
            area.width, area.height
        );
        frame.render_widget(Paragraph::new(message).wrap(Wrap { trim: true }), area);
        return;
    }
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);
    let place = match &app.destination {
        Ok(destination) => destination::label(destination),
        Err(_) => "no destination selected".to_string(),
    };
    let mut title = Line::from(vec![
        Span::styled(" lvpm ", reversed().add_modifier(Modifier::BOLD)),
        Span::raw(format!(" {place}")),
    ]);
    if app.reloading() {
        title.spans.push(Span::raw("   reloading…"));
        if let Some((url, state)) = app.feeds.last() {
            title.spans.push(Span::raw(format!(" {url} {state}")));
        }
    }
    frame.render_widget(Paragraph::new(title), header);

    let (bindings, on_list) = if app.showing_log {
        draw_log(frame, &app.log, body);
        (keys::LOG_FOOTER, false)
    } else if let Some(op) = &app.op {
        draw_op(frame, op, body);
        (op_footer(op), false)
    } else if let Some(detail) = &mut app.detail {
        draw_detail(frame, detail, &app.details, body);
        (keys::DETAIL_FOOTER, false)
    } else {
        let block = Block::bordered();
        let inner = block.inner(body);
        frame.render_widget(block, body);
        match &mut app.listing {
            Listing::Loading => {
                let mut lines = vec![Line::raw("Loading the catalog…"), Line::raw("")];
                lines.extend(
                    app.feeds
                        .iter()
                        .map(|(url, state)| Line::raw(format!("{state:<18} {url}"))),
                );
                frame.render_widget(Paragraph::new(lines), inner)
            }
            Listing::Failed(error) => frame.render_widget(
                Paragraph::new(error.as_str()).wrap(Wrap { trim: false }),
                inner,
            ),
            Listing::Loaded(browse) => draw_list(frame, browse, inner),
        }
        (keys::FOOTER, true)
    };
    // What a key could not do lasts one key, so it comes first and whole.
    // Why nothing can be installed lasts, so the hints go before it.
    let mut line = hints(bindings);
    if let Some(note) = &app.note {
        line.spans.insert(0, Span::raw(format!("{note}  ")));
    } else if let Some(error) = app.destination.as_ref().err().filter(|_| on_list) {
        line.spans.push(Span::raw(
            error.lines().next().unwrap_or_default().to_string(),
        ));
    }
    frame.render_widget(Paragraph::new(line), footer);
}

fn op_footer(op: &Op) -> &'static [&'static Binding] {
    match &op.stage {
        Stage::Planning => keys::PLANNING_FOOTER,
        Stage::PlanFailed(_) => keys::PLAN_FAILED_FOOTER,
        Stage::Preview => keys::PREVIEW_FOOTER,
        Stage::ConfirmDowngrade => keys::DOWNGRADE_FOOTER,
        Stage::Running(progress) if progress.outcome.is_none() => keys::RUNNING_FOOTER,
        Stage::Running(_) => keys::RESULT_FOOTER,
    }
}

/// The install's plan, the question a downgrade asks, or its progress.
fn draw_op(frame: &mut Frame, op: &Op, area: Rect) {
    let mut lines = match &op.stage {
        Stage::Planning => vec![Line::raw(format!("Computing the plan for {}…", op.package))],
        Stage::PlanFailed(error) => {
            let mut lines = vec![Line::raw("The plan could not be made:")];
            lines.extend(error.lines().map(|l| Line::raw(l.to_string())));
            lines
        }
        Stage::Preview => preview_lines(&op.preview),
        Stage::ConfirmDowngrade => {
            let mut lines = preview_lines(&op.preview);
            if let Some(item) = op.preview.downgrade()
                && let Action::Downgrade { from } = &item.action
            {
                let ask = format!("Downgrade {} from {from} to {}?", item.name, item.version);
                lines.insert(0, Line::styled(ask, reversed()));
                lines.insert(1, Line::raw(""));
            }
            lines
        }
        Stage::Running(progress) => progress_lines(&op.package, progress),
    };
    lines.insert(0, Line::raw(""));
    let block = Block::bordered().title(format!(" Install {} ", op.package));
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(paragraph.block(block), area);
}

/// One row per package, the counts, and what the install leaves out.
fn preview_lines(preview: &Preview) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for item in &preview.items {
        let version = match &item.action {
            Action::Upgrade { from } | Action::Downgrade { from } => {
                format!("{from} → {}", item.version)
            }
            Action::New | Action::Already => item.version.clone(),
        };
        let files = match item.files {
            0 => String::new(),
            files => format!("{files} files"),
        };
        let label = item.action.label();
        lines.push(Line::raw(format!(
            "  {label:<18}{:<26}{version:<16}{files}",
            item.name
        )));
    }
    let count = |is: fn(&Action) -> bool| preview.items.iter().filter(|i| is(&i.action)).count();
    lines.push(Line::raw(""));
    lines.push(Line::raw(format!(
        "{} new, {} upgrade, {} downgrade, {} already installed",
        count(|a| matches!(a, Action::New)),
        count(|a| matches!(a, Action::Upgrade { .. })),
        count(|a| matches!(a, Action::Downgrade { .. })),
        count(|a| matches!(a, Action::Already))
    )));
    for name in &preview.missing {
        lines.push(Line::raw(format!(
            "No source has {name}; the install goes on without it."
        )));
    }
    if preview.downgrade().is_some() {
        lines.push(Line::raw(
            "Downgrade: you will be asked again; runs with allow-downgrade.",
        ));
    }
    lines
}

/// A row per phase, then what cancel does now or how the install ended.
fn progress_lines(package: &str, progress: &Progress) -> Vec<Line<'static>> {
    let mut lines: Vec<Line> = PHASES
        .iter()
        .map(|&phase| {
            let state = progress.state(phase);
            let word = match state {
                State::Waiting => String::new(),
                State::Running => "running".into(),
                State::Done => "done".into(),
                State::Stopped => "stopped".into(),
                State::Skipped => match progress.skipped(phase) {
                    Some(reason) => format!("skipped: {}", progress::reason(reason)),
                    None => "skipped".into(),
                },
            };
            let name = progress::name(phase);
            Line::raw(format!("  {} {name:<10}{word}", state.mark()))
        })
        .collect();
    lines.push(Line::raw(""));
    let end = match &progress.outcome {
        None if progress.refused || !progress.cancellable() => {
            "Cancel is no longer possible: package files are being written.".into()
        }
        None if progress.cancelling => "Cancelling…".into(),
        None => "Nothing has been written yet; cancel stops cleanly.".into(),
        Some(Outcome::Done) => format!("Done: {package} is installed."),
        Some(Outcome::Cancelled) => {
            let phase = progress.current().map_or("resolve", progress::name);
            format!("Cancelled during {phase}. No package files were written.")
        }
        Some(Outcome::Failed(error)) => {
            lines.push(Line::raw("× Failed:"));
            lines.extend(error.lines().map(|l| Line::raw(l.to_string())));
            return lines;
        }
    };
    lines.push(Line::raw(end));
    lines
}

/// Every install's lines this session, the newest at the bottom.
// shortcut: only the last screenful shows; add scrolling when a session's
// log outgrows a screen (batches, Slice 8).
fn draw_log(frame: &mut Frame, log: &[Vec<String>], area: Rect) {
    let mut lines: Vec<Line> = log
        .iter()
        .flatten()
        .map(|l| Line::raw(l.as_str()))
        .collect();
    if lines.is_empty() {
        lines.push(Line::raw("No operation has run yet."));
    }
    let block = Block::bordered().title(" Log ");
    let inner = block.inner(area);
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    let count = paragraph.line_count(inner.width) as u16;
    let scroll = count.saturating_sub(inner.height);
    frame.render_widget(paragraph.scroll((scroll, 0)).block(block), area);
}

/// The search and filter line, the banners, and the table.
fn draw_list(frame: &mut Frame, browse: &mut Browse, area: Rect) {
    // A failed reload's banner, then one per source not read.
    let mut banners: Vec<Line> = Vec::new();
    if let Some(error) = &browse.stale {
        let why = "Reloading failed; the list shown is from the cache:";
        banners.push(Line::styled(why, reversed()));
        banners.extend(
            error
                .lines()
                .map(|l| Line::styled(l.to_string(), reversed())),
        );
    }
    let not_read = browse.failed.iter();
    banners.extend(
        not_read.map(|(source, _)| Line::styled(format!("not read: {source}"), reversed())),
    );
    let [status, banner, list] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(banners.len() as u16),
        Constraint::Min(0),
    ])
    .areas(area);
    let cursor = if browse.searching { "_" } else { "" };
    let line = format!(
        "Search: {}{cursor}   Filter [{}]   {} of {}",
        browse.search,
        browse.filter.label(),
        browse.rows.len(),
        browse.packages.len()
    );
    frame.render_widget(Paragraph::new(line), status);
    frame.render_widget(Paragraph::new(banners), banner);

    if browse.rows.is_empty() {
        let message = if browse.packages.is_empty() {
            format!(
                "The catalog is empty. Sources read:\n{}",
                browse.sources.join("\n")
            )
        } else if browse.search.is_empty() {
            format!("No package is {}.", browse.filter.label())
        } else {
            format!("No package matches \"{}\".", browse.search)
        };
        frame.render_widget(Paragraph::new(message).wrap(Wrap { trim: false }), list);
        return;
    }
    // The header row takes one line; the rest is what PgUp and PgDn move by.
    browse.page = list.height.saturating_sub(1).into();
    let rows = browse.rows.iter().map(|&i| {
        let package = &browse.packages[i];
        Row::new([
            package.name.clone(),
            package.source().to_string(),
            package
                .installed
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default(),
            package
                .available
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default(),
            package.status.text(),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Fill(1),
            Constraint::Length(16),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(14),
        ],
    )
    .header(Row::new(["Package", "Source", "Installed", "Available", "Status"]).style(reversed()))
    .row_highlight_style(reversed())
    .highlight_symbol("> ");
    frame.render_stateful_widget(table, list, &mut browse.table);
}

/// The versions on the left, the picked version's fields on the right; the
/// pane Up and Down move has the bold title.
fn draw_detail(frame: &mut Frame, detail: &mut Detail, details: &Details, area: Rect) {
    let [left, right] =
        Layout::horizontal([Constraint::Length(32), Constraint::Min(0)]).areas(area);
    let title = |text: String, focused: bool| match focused {
        true => Line::styled(text, Style::new().add_modifier(Modifier::BOLD)),
        false => Line::raw(text),
    };
    let rows = detail.versions.iter().map(|entry| {
        let installed = detail.installed.as_ref() == Some(&entry.version);
        let year = entry.lv_min.map(detail::labview_year);
        Row::new([
            if installed { "*" } else { "" }.to_string(),
            entry.version.to_string(),
            year.map(|y| format!("LV {y}+")).unwrap_or_default(),
        ])
    });
    let versions = Table::new(
        rows,
        [
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(8),
        ],
    )
    .block(Block::bordered().title(title(" Versions ".into(), detail.focus == Focus::Versions)))
    .row_highlight_style(reversed())
    .highlight_symbol("> ");
    frame.render_stateful_widget(versions, left, &mut detail.table);

    let key = detail.key();
    let text = detail::fields(detail, details.get(&key));
    let block = Block::bordered()
        .title(title(
            format!(" {} {} ", key.0, key.1),
            detail.focus == Focus::Fields,
        ))
        .title_bottom(" PgUp/PgDn scroll ");
    let inner = block.inner(right);
    let paragraph = Paragraph::new(text).wrap(Wrap { trim: false });
    // Stop the scroll where the last line reaches the bottom.
    let lines = paragraph.line_count(inner.width) as u16;
    detail.page = inner.height;
    detail.scroll = detail.scroll.min(lines.saturating_sub(inner.height));
    frame.render_widget(paragraph.scroll((detail.scroll, 0)).block(block), right);
}

fn reversed() -> Style {
    Style::new().add_modifier(Modifier::REVERSED)
}

/// Each key in reverse video, followed by what it does.
fn hints(bindings: &[&Binding]) -> Line<'static> {
    let spans = bindings.iter().flat_map(|binding| {
        [
            Span::styled(binding.key, reversed()),
            Span::raw(format!(" {}  ", binding.action)),
        ]
    });
    Line::from(spans.collect::<Vec<_>>())
}
