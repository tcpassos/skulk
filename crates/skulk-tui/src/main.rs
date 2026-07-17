//! `skulk-tui` — the operator dashboard. A ratatui frontend over the `client`
//! library: it renders menus from the live Manifest and streams events/tasks/loot.

mod app;
mod ui;

use std::time::Duration;

use app::{parse_input, App, Focus, Pending};
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
            KeyCode::Tab => app.focus = Focus::Input,
            KeyCode::Up => app.move_up(),
            KeyCode::Down => app.move_down(),
            KeyCode::Enter => {
                if let Some(template) = app.selected_template() {
                    app.input = template;
                    app.focus = Focus::Input;
                }
            }
            _ => {}
        },
        Focus::Input => match key.code {
            KeyCode::Esc | KeyCode::Tab => app.focus = Focus::Modules,
            KeyCode::Backspace => {
                app.input.pop();
            }
            KeyCode::Char(c) => app.input.push(c),
            KeyCode::Enter => {
                let line = app.input.trim().to_string();
                app.input.clear();
                if line.is_empty() {
                    return;
                }
                match parse_input(&line) {
                    Ok((command, pending)) => match sender.send(command).await {
                        Ok(id) => {
                            app.pending.insert(id, pending);
                            app.log(format!("> {line}"));
                        }
                        Err(e) => app.log(format!("send failed: {e}")),
                    },
                    Err(e) => app.log(format!("! {e}")),
                }
            }
            _ => {}
        },
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
