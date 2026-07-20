//! Rendering. Pure function of `&App` -> frame.

use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, Paragraph};
use ratatui::Frame;

use crate::app::{short, App, Focus, Row};

const ACCENT: Color = Color::Rgb(0xef, 0x77, 0x34); // Skulk rust-orange
const GROUP: Color = Color::Cyan;
const DIM: Color = Color::DarkGray;

pub fn render(frame: &mut Frame, app: &App) {
    let outer = Layout::vertical([
        Constraint::Length(3), // header
        Constraint::Min(0),    // body
        Constraint::Length(3), // input
        Constraint::Length(1), // footer
    ])
    .split(frame.area());

    let body = Layout::horizontal([
        Constraint::Percentage(28),
        Constraint::Percentage(44),
        Constraint::Percentage(28),
    ])
    .split(outer[1]);

    let middle =
        Layout::vertical([Constraint::Percentage(45), Constraint::Percentage(55)]).split(body[1]);
    let right = Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]).split(body[2]);

    header(frame, app, outer[0]);
    modules(frame, app, body[0]);
    // The middle-top panel is the module help, the editable form once opened,
    // or a fetched loot item's content -- all mutually exclusive, so they
    // share the larger panel instead of squeezing into the small LOOT list.
    if app.focus == Focus::Form {
        form_view(frame, app, middle[0]);
    } else if app.loot_content.is_some() {
        loot_content_view(frame, app, middle[0]);
    } else {
        detail(frame, app, middle[0]);
    }
    events(frame, app, middle[1]);
    tasks(frame, app, right[0]);
    loot(frame, app, right[1]);
    command_bar(frame, app, outer[2]);
    footer(frame, outer[3]);
}

/// The field-by-field editor rendered while `Focus::Form` is active. Each declared
/// param is a labelled row; the focused row shows a block caret, and empty fields
/// show their declared default/example as a dim hint.
fn form_view(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let mut lines: Vec<Line> = Vec::new();
    if let Some(form) = &app.form {
        lines.push(Line::from(vec![
            Span::styled(
                format!("{} ", form.module.0),
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(form.action.clone(), Style::new().add_modifier(Modifier::BOLD)),
        ]));
        lines.push(Line::styled(
            "Tab: next field   Enter: next / run   Esc: cancel",
            Style::new().fg(DIM),
        ));
        lines.push(Line::raw(""));

        if form.fields.is_empty() {
            lines.push(Line::styled("no params  -  press Enter to run", Style::new().fg(DIM)));
        } else {
            for (i, f) in form.fields.iter().enumerate() {
                let focused = i == form.cursor;
                let req = if f.required { "*" } else { " " };
                let ty = f.type_hint.clone().unwrap_or_default();
                let mut spans = vec![
                    Span::styled(format!(" {req} "), Style::new().fg(ACCENT)),
                    Span::styled(format!("{:<12}", f.name), Style::new().add_modifier(Modifier::BOLD)),
                    Span::styled(format!("{ty:<9} "), Style::new().fg(GROUP)),
                ];
                let value_style = if focused {
                    Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
                } else {
                    Style::new()
                };
                spans.push(Span::styled(f.value.clone(), value_style));
                if focused {
                    // Block caret (no hardware cursor: robust against line wraps).
                    spans.push(Span::styled(" ", Style::new().add_modifier(Modifier::REVERSED)));
                }
                if f.value.is_empty() {
                    let hint = f
                        .default
                        .as_ref()
                        .map(|d| format!("default: {d}"))
                        .or_else(|| f.example.as_ref().map(|e| format!("e.g. {e}")));
                    if let Some(hint) = hint {
                        spans.push(Span::styled(format!("  {hint}"), Style::new().fg(DIM)));
                    }
                }
                lines.push(Line::from(spans));
            }
        }
    }
    frame.render_widget(Paragraph::new(lines).block(Block::bordered().title(" FORM ")), area);
}

/// Help for the selected module/action, rendered from its declared `ParamSpec`s.
fn detail(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let mut lines: Vec<Line> = Vec::new();
    if let Some(Row::Module { id, action, description, params, available }) = app.selected_row() {
        let mut title = vec![
            Span::styled(format!("{} ", id.0), Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(action.clone(), Style::new().add_modifier(Modifier::BOLD)),
        ];
        if !*available {
            title.push(Span::styled("  (unavailable here)", Style::new().fg(DIM)));
        }
        lines.push(Line::from(title));
        if let Some(d) = description {
            lines.push(Line::styled(d.clone(), Style::new().fg(DIM)));
        }
        lines.push(Line::raw(""));

        if params.is_empty() {
            lines.push(Line::styled("no declared params  (Enter runs it directly)", Style::new().fg(DIM)));
        } else {
            lines.push(Line::styled("params  (* = required)", Style::new().add_modifier(Modifier::BOLD)));
            for p in params {
                let req = if p.required { "*" } else { " " };
                let ty = p.type_hint.clone().unwrap_or_default();
                let mut spans = vec![
                    Span::styled(format!(" {req} "), Style::new().fg(ACCENT)),
                    Span::styled(format!("{:<12}", p.name), Style::new().add_modifier(Modifier::BOLD)),
                    Span::styled(format!("{ty:<9} "), Style::new().fg(GROUP)),
                ];
                if let Some(d) = &p.description {
                    spans.push(Span::raw(d.clone()));
                }
                lines.push(Line::from(spans));
                let mut extra = String::new();
                if let Some(dv) = &p.default {
                    extra.push_str(&format!("default: {dv}   "));
                }
                if let Some(ex) = &p.example {
                    extra.push_str(&format!("e.g. {ex}"));
                }
                if !extra.is_empty() {
                    lines.push(Line::styled(format!("                  {extra}"), Style::new().fg(DIM)));
                }
            }
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                "Enter: open an editable form for these params",
                Style::new().fg(DIM),
            ));
        }
    } else {
        lines.push(Line::styled("select a module (Up/Down)", Style::new().fg(DIM)));
    }
    frame.render_widget(Paragraph::new(lines).block(Block::bordered().title(" DETAIL ")), area);
}

fn header(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let hb = app
        .last_heartbeat
        .map(|t| format!("{}s ago", t.elapsed().as_secs()))
        .unwrap_or_else(|| "-".to_string());
    let line = Line::from(vec![
        Span::styled(" Skulk ", Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
        Span::raw(format!(
            "{} · {} · proto v{} · hb {} · {}",
            app.implant.id, app.implant.hardware, app.protocol, hb, app.addr
        )),
    ]);
    frame.render_widget(Paragraph::new(line).block(Block::bordered()), area);
}

fn modules(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let items: Vec<ListItem> = app
        .rows
        .iter()
        .enumerate()
        .map(|(i, row)| match row {
            Row::Group(g) => ListItem::new(Line::styled(
                format!(" {g}/"),
                Style::new().fg(GROUP).add_modifier(Modifier::BOLD),
            )),
            Row::Module { id, action, available, .. } => {
                let name = id.0.split_once('.').map(|(_, n)| n).unwrap_or(&id.0);
                let marker = if *available { "on" } else { "!!" };
                let text = format!("   {name}  [{action}]  {marker}");
                let mut style = if *available { Style::new() } else { Style::new().fg(DIM) };
                if i == app.selected {
                    style = Style::new().fg(ACCENT).add_modifier(Modifier::REVERSED);
                }
                ListItem::new(Line::styled(text, style))
            }
            Row::Loot(entry) => {
                let text = format!("   {}  {:?}  {} B", entry.key, entry.kind, entry.size);
                let style = if i == app.selected {
                    Style::new().fg(ACCENT).add_modifier(Modifier::REVERSED)
                } else {
                    Style::new()
                };
                ListItem::new(Line::styled(text, style))
            }
        })
        .collect();
    frame.render_widget(List::new(items).block(Block::bordered().title(" MODULES ")), area);
}

fn events(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    // newest first
    let items: Vec<ListItem> =
        app.events.iter().rev().map(|l| ListItem::new(Line::raw(l.clone()))).collect();
    frame.render_widget(List::new(items).block(Block::bordered().title(" EVENTS ")), area);
}

fn tasks(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let items: Vec<ListItem> = app
        .tasks
        .iter()
        .map(|t| {
            let status = t
                .status
                .map(|s| format!("{s:?}"))
                .unwrap_or_else(|| t.note.clone());
            ListItem::new(Line::raw(format!("{} {} {}", short(t.id), bar(t.pct), status)))
        })
        .collect();
    frame.render_widget(List::new(items).block(Block::bordered().title(" TASKS ")), area);
}

/// A passive mirror of the loot index -- browsing/fetching happens through
/// the MODULES tree above (a "loot" group; see `App::rebuild_loot_rows`),
/// same as everything else, so this panel has no selection of its own.
fn loot(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let items: Vec<ListItem> = app
        .loot
        .iter()
        .map(|e| ListItem::new(Line::raw(format!("{}  {:?}  {} B", e.key, e.kind, e.size))))
        .collect();
    frame.render_widget(List::new(items).block(Block::bordered().title(" LOOT ")), area);
}

/// One fetched loot item's content, in place of the DETAIL/FORM panel.
/// Best-effort UTF-8; anything else just says so rather than dumping raw
/// bytes.
fn loot_content_view(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let Some(content) = &app.loot_content else { return };
    let mut lines = vec![
        Line::from(vec![
            Span::styled(format!("{} ", content.key), Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(format!("{:?}", content.kind), Style::new().fg(GROUP)),
        ]),
        Line::styled(format!("{} B   Up/Down: scroll   Esc: back to list", content.bytes.len()), Style::new().fg(DIM)),
        Line::raw(""),
    ];
    match std::str::from_utf8(&content.bytes) {
        Ok(text) => lines.extend(text.lines().map(|l| Line::raw(l.to_string()))),
        Err(_) => lines.push(Line::styled("(binary content, not previewable)", Style::new().fg(DIM))),
    }
    let para = Paragraph::new(lines)
        .scroll((app.loot_scroll, 0))
        .block(Block::bordered().title(" LOOT CONTENT "));
    frame.render_widget(para, area);
}

/// A live preview of the command the open form will send (or a hint when idle).
fn command_bar(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let focused = app.focus == Focus::Form;
    let text = match &app.form {
        Some(form) => {
            let mut s = format!("{} {}", form.module.0, form.action);
            for f in &form.fields {
                let v = f.value.trim();
                if !v.is_empty() {
                    s.push_str(&format!(" {}={}", f.name, v));
                }
            }
            s
        }
        None => {
            "Enter: fill the form / view loot    r: refresh   l: refresh loot   p: ping   c: stop task"
                .to_string()
        }
    };
    let style = if focused { Style::new().fg(ACCENT) } else { Style::new().fg(DIM) };
    let para = Paragraph::new(Line::from(vec![Span::raw("> "), Span::raw(text)]))
        .style(style)
        .block(Block::bordered().title(" COMMAND "));
    frame.render_widget(para, area);
}

fn footer(frame: &mut Frame, area: ratatui::layout::Rect) {
    let line = Line::styled(
        "  Up/Down: tree   Enter: form / run / view loot   Tab: fields   Esc: cancel/back   \
         c: stop task   r: refresh   l: refresh loot   p: ping   Ctrl+C: quit",
        Style::new().fg(DIM),
    );
    frame.render_widget(Paragraph::new(line), area);
}

fn bar(pct: Option<u8>) -> String {
    match pct {
        Some(p) => {
            let filled = (p as usize / 10).min(10);
            format!("[{}{}] {p:>3}%", "#".repeat(filled), "-".repeat(10 - filled))
        }
        None => "[  ...   ]     ".to_string(),
    }
}
