//! TUI state and the reducer that applies incoming envelopes. No rendering here.

use std::collections::HashMap;
use std::time::Instant;

use contract::*;

#[derive(PartialEq)]
pub enum Focus {
    Modules,
    Input,
}

/// What a sent command is, so its correlated Result can be routed.
pub enum Pending {
    Describe,
    Loot,
    Invoke,
}

pub struct TaskRow {
    pub id: TaskId,
    pub pct: Option<u8>,
    pub status: Option<TaskStatus>,
    pub note: String,
}

/// One row in the left module tree.
pub enum Row {
    Group(String),
    Module {
        id: ModuleId,
        action: String,
        available: bool,
        description: Option<String>,
        params: Vec<ParamSpec>,
    },
}

pub struct App {
    pub implant: ImplantInfo,
    pub protocol: u16,
    pub capabilities: Vec<Capability>,
    pub rows: Vec<Row>,
    pub selected: usize,
    pub focus: Focus,
    pub input: String,
    pub tasks: Vec<TaskRow>,
    pub events: Vec<String>,
    pub loot: Vec<LootEntry>,
    pub last_heartbeat: Option<Instant>,
    pub started: Instant,
    pub pending: HashMap<MessageId, Pending>,
    /// Commands the reducer wants sent (e.g. refresh loot after a task); the main
    /// loop drains and sends these.
    pub outbox: Vec<(Command, Pending)>,
    pub addr: String,
    pub should_quit: bool,
}

impl App {
    pub fn new(addr: String) -> Self {
        App {
            implant: ImplantInfo { id: String::new(), hardware: String::new(), firmware: String::new() },
            protocol: 0,
            capabilities: Vec::new(),
            rows: Vec::new(),
            selected: 0,
            focus: Focus::Modules,
            input: String::new(),
            tasks: Vec::new(),
            events: Vec::new(),
            loot: Vec::new(),
            last_heartbeat: None,
            started: Instant::now(),
            pending: HashMap::new(),
            outbox: Vec::new(),
            addr,
            should_quit: false,
        }
    }

    /// Rebuild the module tree from a fresh manifest.
    pub fn set_manifest(&mut self, m: Manifest) {
        self.implant = m.implant;
        self.protocol = m.protocol;
        self.capabilities = m.capabilities;

        let mut modules = m.modules;
        modules.sort_by(|a, b| a.id.0.cmp(&b.id.0));

        let mut rows = Vec::new();
        let mut group = String::new();
        for md in &modules {
            let g = md.id.0.split_once('.').map(|(g, _)| g).unwrap_or("").to_string();
            if g != group {
                rows.push(Row::Group(g.clone()));
                group = g;
            }
            let available = md.requires.iter().all(|c| self.capabilities.contains(c));
            if md.actions.is_empty() {
                rows.push(Row::Module {
                    id: md.id.clone(),
                    action: String::new(),
                    available,
                    description: None,
                    params: Vec::new(),
                });
            }
            for action in &md.actions {
                rows.push(Row::Module {
                    id: md.id.clone(),
                    action: action.name.clone(),
                    available,
                    description: action.description.clone(),
                    params: action.params.clone(),
                });
            }
        }
        self.rows = rows;
        self.selected = self
            .rows
            .iter()
            .position(|r| matches!(r, Row::Module { .. }))
            .unwrap_or(0);
    }

    pub fn move_down(&mut self) {
        let mut i = self.selected;
        while i + 1 < self.rows.len() {
            i += 1;
            if matches!(self.rows[i], Row::Module { .. }) {
                self.selected = i;
                break;
            }
        }
    }

    pub fn move_up(&mut self) {
        let mut i = self.selected;
        while i > 0 {
            i -= 1;
            if matches!(self.rows[i], Row::Module { .. }) {
                self.selected = i;
                break;
            }
        }
    }

    /// The `Row` under the cursor, if it is a module (for the detail panel).
    pub fn selected_row(&self) -> Option<&Row> {
        self.rows.get(self.selected).filter(|r| matches!(r, Row::Module { .. }))
    }

    /// A pre-filled command line for the selected module: `<id> <action> ` plus a
    /// `name=` stub for each required parameter, ready for the operator to fill.
    pub fn selected_template(&self) -> Option<String> {
        match self.rows.get(self.selected) {
            Some(Row::Module { id, action, params, .. }) => {
                let mut line = format!("{} {} ", id.0, action);
                for p in params.iter().filter(|p| p.required) {
                    line.push_str(&format!("{}=", p.name));
                }
                Some(line)
            }
            _ => None,
        }
    }

    pub fn log(&mut self, line: String) {
        let secs = self.started.elapsed().as_secs();
        self.events.push(format!("[{secs:>4}s] {line}"));
        let len = self.events.len();
        if len > 300 {
            self.events.drain(0..len - 300);
        }
    }

    /// Fold one inbound envelope into the state.
    pub fn apply(&mut self, env: Envelope) {
        let correlate = env.correlate;
        match env.body {
            Body::Ack(a) => {
                self.upsert_task(a.task, None, "started".to_string());
                self.log(format!("task {} started", short(a.task)));
            }
            Body::Result(r) => match correlate.and_then(|id| self.pending.remove(&id)) {
                Some(Pending::Describe) => {
                    if let Ok(m) = serde_json::from_value::<Manifest>(r.output.0.clone()) {
                        self.set_manifest(m);
                        self.log("manifest refreshed".to_string());
                    }
                }
                Some(Pending::Loot) => {
                    if let Ok(list) = serde_json::from_value::<Vec<LootEntry>>(r.output.0.clone()) {
                        self.loot = list;
                        self.log(format!("loot: {} item(s)", self.loot.len()));
                    }
                }
                _ => {
                    self.finish_task(r.task, r.status);
                    let out = summarize_output(&r.output.0);
                    let tag = if r.status == TaskStatus::Ok { "=" } else { "x" };
                    if out.is_empty() {
                        self.log(format!("{tag} task {} {:?}", short(r.task), r.status));
                    } else {
                        self.log(format!("{tag} task {} {:?}: {out}", short(r.task), r.status));
                    }
                    // A task may have stored loot; refresh the panel.
                    self.outbox.push((Command::Loot(LootQuery::default()), Pending::Loot));
                }
            },
            Body::Error(e) => {
                if let Some(id) = correlate {
                    self.pending.remove(&id);
                }
                self.log(format!("! {:?}: {}", e.code, e.message));
            }
            Body::Event(ev) => match ev {
                Event::Progress { task, pct, note } => self.upsert_task(task, pct, note),
                Event::Log { level, source, msg } => self.log(format!("[{level:?}] {source} {msg}")),
                Event::Alert { severity, source, msg } => {
                    self.log(format!("! [{severity:?}] {source} {msg}"))
                }
                Event::LootStored { key, kind, size } => {
                    self.log(format!("+ loot {key} ({kind:?}, {size} B)"));
                    if !self.loot.iter().any(|e| e.key == key) {
                        self.loot.push(LootEntry { key, kind, size });
                    }
                }
                Event::Heartbeat { .. } => self.last_heartbeat = Some(Instant::now()),
                Event::Sensor { source, .. } => self.log(format!("sensor {source}")),
                Event::ViewManifest(_) => {}
            },
            Body::Command(_) => {}
        }
    }

    fn upsert_task(&mut self, id: TaskId, pct: Option<u8>, note: String) {
        if let Some(t) = self.tasks.iter_mut().find(|t| t.id == id) {
            if pct.is_some() {
                t.pct = pct;
            }
            t.note = note;
        } else {
            self.tasks.insert(0, TaskRow { id, pct, status: None, note });
            self.tasks.truncate(20);
        }
    }

    fn finish_task(&mut self, id: TaskId, status: TaskStatus) {
        if let Some(t) = self.tasks.iter_mut().find(|t| t.id == id) {
            t.status = Some(status);
            t.pct = Some(100);
        } else {
            self.tasks.insert(0, TaskRow { id, pct: Some(100), status: Some(status), note: String::new() });
        }
    }
}

pub fn short(id: TaskId) -> String {
    id.0.to_string().chars().take(8).collect()
}

/// One-line summary of a task's output for the event log: the error string if
/// the module failed, otherwise compact JSON (truncated).
fn summarize_output(v: &serde_json::Value) -> String {
    if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
        return err.to_string();
    }
    if v.is_null() {
        return String::new();
    }
    let s = serde_json::to_string(v).unwrap_or_default();
    let clipped: String = s.chars().take(240).collect();
    if clipped.len() < s.len() {
        format!("{clipped}…")
    } else {
        clipped
    }
}

/// Parse a CLI-style input line into a command (module-first, or a reserved verb).
pub fn parse_input(line: &str) -> Result<(Command, Pending), String> {
    let toks: Vec<&str> = line.split_whitespace().collect();
    let first = *toks.first().ok_or("empty command")?;

    if first.contains('.') {
        let action = toks
            .get(1)
            .ok_or_else(|| format!("usage: {first} <action> [key=value ...]"))?;
        let params = build_params(toks.get(2..).unwrap_or(&[]))?;
        Ok((
            Command::Invoke(Invoke {
                module: ModuleId::from(first),
                action: action.to_string(),
                params,
                timeout_ms: None,
            }),
            Pending::Invoke,
        ))
    } else {
        match first {
            "describe" | "modules" => Ok((Command::Describe, Pending::Describe)),
            "loot" => Ok((Command::Loot(LootQuery::default()), Pending::Loot)),
            "ping" => Ok((Command::Ping, Pending::Invoke)),
            "shutdown" => {
                let wipe = toks.iter().any(|t| *t == "--wipe");
                let mode = if wipe { ShutdownMode::Wipe } else { ShutdownMode::Graceful };
                Ok((Command::Shutdown { mode }, Pending::Invoke))
            }
            other => Err(format!("unknown command '{other}'")),
        }
    }
}

fn build_params(pairs: &[&str]) -> Result<RawParams, String> {
    let mut map = serde_json::Map::new();
    for pair in pairs {
        let (key, raw) = pair
            .split_once('=')
            .ok_or_else(|| format!("bad parameter '{pair}' (expected key=value)"))?;
        let value = serde_json::from_str::<serde_json::Value>(raw)
            .unwrap_or_else(|_| serde_json::Value::String(raw.to_string()));
        map.insert(key.to_string(), value);
    }
    Ok(RawParams(serde_json::Value::Object(map)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_manifest() -> Manifest {
        Manifest {
            protocol: 1,
            implant: ImplantInfo { id: "t".into(), hardware: "hw".into(), firmware: "0".into() },
            modules: vec![
                ModuleDescriptor {
                    id: ModuleId::from("net.ports"),
                    version: "0".into(),
                    tactic: Some(Tactic::Discovery),
                    actions: vec![ActionSpec { name: "scan".into(), description: None, params: vec![], params_schema: None }],
                    requires: vec![],
                },
                ModuleDescriptor {
                    id: ModuleId::from("wifi.deauth"),
                    version: "0".into(),
                    tactic: None,
                    actions: vec![ActionSpec { name: "start".into(), description: None, params: vec![], params_schema: None }],
                    requires: vec![Capability::MonitorMode],
                },
            ],
            capabilities: vec![], // no MonitorMode -> wifi.deauth unavailable
        }
    }

    #[test]
    fn manifest_builds_grouped_tree_with_availability() {
        let mut app = App::new("x".into());
        app.set_manifest(sample_manifest());
        assert!(matches!(&app.rows[0], Row::Group(g) if g == "net"));
        match &app.rows[1] {
            Row::Module { available, .. } => assert!(*available),
            _ => panic!("expected net.ports module row"),
        }
        assert!(matches!(&app.rows[2], Row::Group(g) if g == "wifi"));
        match &app.rows[3] {
            Row::Module { available, .. } => assert!(!*available, "wifi.deauth needs MonitorMode"),
            _ => panic!("expected wifi.deauth module row"),
        }
        assert_eq!(app.selected, 1, "cursor starts on the first module row");
    }

    #[test]
    fn navigation_skips_group_rows() {
        let mut app = App::new("x".into());
        app.set_manifest(sample_manifest());
        assert_eq!(app.selected, 1);
        app.move_down(); // skips Group(wifi) at index 2
        assert_eq!(app.selected, 3);
        app.move_up();
        assert_eq!(app.selected, 1);
    }

    #[test]
    fn module_invoke_infers_param_types() {
        let (cmd, _) =
            parse_input("net.ports scan target=127.0.0.1 ports=[1,1024] timeout=200").unwrap();
        match cmd {
            Command::Invoke(inv) => {
                assert_eq!(inv.module, ModuleId::from("net.ports"));
                assert_eq!(inv.action, "scan");
                assert_eq!(inv.params.0["target"], serde_json::json!("127.0.0.1"));
                assert_eq!(inv.params.0["ports"], serde_json::json!([1, 1024]));
                assert_eq!(inv.params.0["timeout"], serde_json::json!(200));
            }
            _ => panic!("expected an invoke command"),
        }
    }

    #[test]
    fn reserved_verbs_and_unknown() {
        assert!(matches!(parse_input("describe").unwrap().0, Command::Describe));
        assert!(matches!(parse_input("loot").unwrap().0, Command::Loot(_)));
        assert!(parse_input("bogus").is_err());
    }

    #[test]
    fn apply_folds_ack_progress_and_heartbeat() {
        let mut app = App::new("x".into());
        let task = TaskId::new();
        app.apply(Envelope::new(Body::Ack(Ack { task }), 0));
        app.apply(Envelope::new(
            Body::Event(Event::Progress { task, pct: Some(50), note: "half".into() }),
            0,
        ));
        assert_eq!(app.tasks.len(), 1);
        assert_eq!(app.tasks[0].pct, Some(50));
        assert!(app.last_heartbeat.is_none());
        app.apply(Envelope::new(Body::Event(Event::Heartbeat { seq: 1 }), 0));
        assert!(app.last_heartbeat.is_some());
    }
}

