//! `skulk-tui` — the operator dashboard. A ratatui frontend over the `client`
//! library: it renders menus from the live Manifest and streams events/tasks/loot.

mod app;
mod ui;

use std::time::Duration;

use app::{short, App, Focus, Pending};
use client::{Client, Sender};
use contract::{Command, LootQuery};
use crossterm::event::{Event as CEvent, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures::StreamExt;
use tokio::sync::mpsc;

#[tokio::main]
async fn main() {
    let addr = connect_addr();

    let mut client = match Client::connect(&addr).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("skulk-tui: cannot connect to {addr}: {e}");
            std::process::exit(1);
        }
    };
    let manifest = match client.describe().await {
        Ok(m) => m,
        Err(e) => {
            eprintln!("skulk-tui: describe failed: {e}");
            std::process::exit(1);
        }
    };

    let (mut sender, mut receiver) = client.split();

    // Reader task: drain the connection into a channel the UI loop selects on.
    let (env_tx, mut env_rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Ok(Some(env)) = receiver.recv().await {
            if env_tx.send(env).is_err() {
                break;
            }
        }
    });

    let mut app = App::new(addr);
    app.set_manifest(manifest);
    app.log("connected".to_string());

    // Prime the loot panel with whatever is already stored on the device.
    if let Ok(id) = sender.send(Command::Loot(LootQuery::default())).await {
        app.pending.insert(id, Pending::Loot);
    }

    let mut terminal = ratatui::init();
    let mut term_events = EventStream::new();
    let mut redraw = tokio::time::interval(Duration::from_millis(500));

    loop {
        if terminal.draw(|f| ui::render(f, &app)).is_err() {
            break;
        }
        if app.should_quit {
            break;
        }
        tokio::select! {
            maybe = term_events.next() => {
                if let Some(Ok(CEvent::Key(key))) = maybe {
                    if key.kind == KeyEventKind::Press {
                        handle_key(&mut app, key, &mut sender).await;
                    }
                }
            }
            Some(env) = env_rx.recv() => {
                app.apply(env);
                // Flush any commands the reducer queued (e.g. loot refresh).
                while let Some((command, pending)) = app.outbox.pop() {
                    if let Ok(id) = sender.send(command).await {
                        app.pending.insert(id, pending);
                    }
                }
            }
            _ = redraw.tick() => {}
        }
    }

    ratatui::restore();
}

async fn handle_key(app: &mut App, key: KeyEvent, sender: &mut Sender) {
    // Global quit.
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('q'))
    {
        app.should_quit = true;
        return;
    }

    match app.focus {
        Focus::Modules => match key.code {
            KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
            KeyCode::Up => app.move_up(),
            KeyCode::Down => app.move_down(),
            // Open the field-by-field form for the selected module action.
            KeyCode::Enter | KeyCode::Tab => {
                app.open_form();
            }
            // Reserved verbs (the free-text command line is gone): quick refreshes.
            KeyCode::Char('r') => {
                send(app, sender, Command::Describe, Pending::Describe, "describe").await
            }
            // Move focus into the loot panel and refresh it.
            KeyCode::Char('l') => {
                app.focus = Focus::Loot;
                send(app, sender, Command::Loot(LootQuery::default()), Pending::Loot, "loot").await;
            }
            KeyCode::Char('p') => send(app, sender, Command::Ping, Pending::Invoke, "ping").await,
            // Cancel is fire-and-forget: the core doesn't ack it directly, the
            // targeted task's own Result (status Cancelled) arrives later
            // correlated to its *original* Invoke, already tracked below.
            KeyCode::Char('c') => match app.running_task() {
                Some(task) => match sender.send(Command::Cancel { task }).await {
                    Ok(_) => app.log(format!("> cancel {}", short(task))),
                    Err(e) => app.log(format!("cancel failed: {e}")),
                },
                None => app.log("no running task to cancel".to_string()),
            },
            _ => {}
        },
        Focus::Form => match key.code {
            KeyCode::Esc => app.close_form(),
            KeyCode::Tab => app.form_next(),
            KeyCode::BackTab => app.form_prev(),
            KeyCode::Backspace => app.form_backspace(),
            // Advance to the next field, or (on the last field) send the Invoke.
            KeyCode::Enter => {
                if let Some(command) = app.form_enter() {
                    let label = match &command {
                        Command::Invoke(inv) => format!("{} {}", inv.module.0, inv.action),
                        _ => String::new(),
                    };
                    send(app, sender, command, Pending::Invoke, &label).await;
                    app.close_form();
                }
            }
            KeyCode::Char(c) => app.form_char(c),
            _ => {}
        },
        Focus::Loot => match app.loot_content.is_some() {
            // Viewing one fetched item's content: Up/Down scroll it.
            true => match key.code {
                KeyCode::Esc => app.close_loot_content(),
                KeyCode::Up => app.loot_scroll_up(),
                KeyCode::Down => app.loot_scroll_down(),
                _ => {}
            },
            // Browsing the list: Up/Down move the cursor, Enter fetches.
            false => match key.code {
                KeyCode::Esc => app.focus = Focus::Modules,
                KeyCode::Up => app.loot_move_up(),
                KeyCode::Down => app.loot_move_down(),
                KeyCode::Enter => {
                    if let Some(command) = app.fetch_selected_loot() {
                        send(app, sender, command, Pending::LootFetch, "loot fetch").await;
                    }
                }
                _ => {}
            },
        },
    }
}

/// Send a command, record its pending kind so the correlated result is routed,
/// and echo it into the event log.
async fn send(app: &mut App, sender: &mut Sender, command: Command, pending: Pending, label: &str) {
    match sender.send(command).await {
        Ok(id) => {
            app.pending.insert(id, pending);
            if !label.is_empty() {
                app.log(format!("> {label}"));
            }
        }
        Err(e) => app.log(format!("send failed: {e}")),
    }
}

/// Resolve the controller address: `--connect ADDR`, a positional, or the default.
fn connect_addr() -> String {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(i) = args.iter().position(|a| a == "--connect") {
        if let Some(addr) = args.get(i + 1) {
            return addr.clone();
        }
    }
    args.into_iter()
        .find(|a| !a.starts_with('-'))
        .unwrap_or_else(|| "127.0.0.1:9000".to_string())
}
