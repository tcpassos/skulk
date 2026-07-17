//! Rendering. Pure function of `&App` -> frame.

use ratatui::layout::{Constraint, Layout, Position};
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
    detail(frame, app, middle[0]);
    events(frame, app, middle[1]);
    tasks(frame, app, right[0]);
    loot(frame, app, right[1]);
    input(frame, app, outer[2]);
    footer(frame, outer[3]);
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
            lines.push(Line::styled("no declared params  (use key=value)", Style::new().fg(DIM)));
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
                "Enter: pre-fill required params into the command line",
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

fn loot(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let items: Vec<ListItem> = app
        .loot
        .iter()
        .map(|e| ListItem::new(Line::raw(format!("{}  {:?}  {} B", e.key, e.kind, e.size))))
        .collect();
    frame.render_widget(List::new(items).block(Block::bordered().title(" LOOT ")), area);
}

fn input(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let focused = app.focus == Focus::Input;
    let style = if focused { Style::new().fg(ACCENT) } else { Style::new().fg(DIM) };
    let para = Paragraph::new(Line::from(vec![Span::raw("> "), Span::raw(app.input.as_str())]))
        .style(style)
        .block(Block::bordered().title(" COMMAND "));
    frame.render_widget(para, area);
    if focused {
        let x = area.x + 3 + app.input.chars().count() as u16;
        frame.set_cursor_position(Position::new(x, area.y + 1));
    }
}

fn footer(frame: &mut Frame, area: ratatui::layout::Rect) {
    let line = Line::styled(
        "  Tab: focus   Up/Down: modules   Enter: select/run   Backspace: edit   Ctrl+C: quit",
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
