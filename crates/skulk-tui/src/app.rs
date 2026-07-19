//! TUI state and the reducer that applies incoming envelopes. No rendering here.

use std::collections::HashMap;
use std::time::Instant;

use contract::*;

#[derive(PartialEq)]
pub enum Focus {
    Modules,
    Form,
}

/// What a sent command is, so its correlated Result can be routed.
pub enum Pending {
    Describe,
    Loot,
    Invoke,
}

/// One editable field of a [`Form`], derived from an action's [`ParamSpec`].
/// The `default`/`example` are carried as *hints* only: the field starts empty
/// (many declared defaults are prose like `"common ports"`, not literal input),
/// and an empty field is simply omitted from the invoke so the module applies
/// its own real default.
pub struct Field {
    pub name: String,
    pub required: bool,
    pub type_hint: Option<String>,
    pub default: Option<String>,
    pub example: Option<String>,
    pub value: String,
}

/// A field-by-field editor for one module action, built from its `ParamSpec`s.
/// Replaces the old free-text command line: the operator fills declared params
/// directly instead of typing a `key=value` string.
pub struct Form {
    pub module: ModuleId,
    pub action: String,
    pub fields: Vec<Field>,
    /// Index of the focused field.
    pub cursor: usize,
}

impl Form {
    /// The cursor is on the final field (or the form has no fields) — the point
    /// at which Enter submits rather than advancing.
    fn on_last(&self) -> bool {
        self.cursor + 1 >= self.fields.len()
    }

    /// Compose the `Invoke` from the current field values. Empty fields are
    /// omitted (the module keeps its own default); non-empty values infer their
    /// JSON type, falling back to a string — the same rule the CLI uses.
    fn to_invoke(&self) -> Invoke {
        let mut map = serde_json::Map::new();
        for f in &self.fields {
            let raw = f.value.trim();
            if raw.is_empty() {
                continue;
            }
            let value = serde_json::from_str::<serde_json::Value>(raw)
                .unwrap_or_else(|_| serde_json::Value::String(raw.to_string()));
            map.insert(f.name.clone(), value);
        }
        Invoke {
            module: self.module.clone(),
            action: self.action.clone(),
            params: RawParams(serde_json::Value::Object(map)),
            timeout_ms: None,
        }
    }
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
    /// The editable form for the module under the cursor, once opened.
    pub form: Option<Form>,
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
            form: None,
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

    /// Open an editable form for the module action under the cursor and focus it,
    /// one field per declared `ParamSpec`. Fields start empty (defaults are shown
    /// as hints, not injected). Returns `false` if the cursor is not on a module.
    /// Replaces the old `selected_template()` command-line stub.
    pub fn open_form(&mut self) -> bool {
        let (module, action, fields) = {
            let Some(Row::Module { id, action, params, .. }) = self.rows.get(self.selected) else {
                return false;
            };
            let fields = params
                .iter()
                .map(|p| Field {
                    name: p.name.clone(),
                    required: p.required,
                    type_hint: p.type_hint.clone(),
                    default: p.default.clone(),
                    example: p.example.clone(),
                    value: String::new(),
                })
                .collect();
            (id.clone(), action.clone(), fields)
        };
        self.form = Some(Form { module, action, fields, cursor: 0 });
        self.focus = Focus::Form;
        true
    }

    /// Discard the open form and return focus to the module tree.
    pub fn close_form(&mut self) {
        self.form = None;
        self.focus = Focus::Modules;
    }

    /// Move focus to the next field, wrapping to the first.
    pub fn form_next(&mut self) {
        if let Some(f) = self.form.as_mut() {
            if !f.fields.is_empty() {
                f.cursor = (f.cursor + 1) % f.fields.len();
            }
        }
    }

    /// Move focus to the previous field, wrapping to the last.
    pub fn form_prev(&mut self) {
        if let Some(f) = self.form.as_mut() {
            if !f.fields.is_empty() {
                f.cursor = (f.cursor + f.fields.len() - 1) % f.fields.len();
            }
        }
    }

    /// Type a character into the focused field.
    pub fn form_char(&mut self, c: char) {
        if let Some(f) = self.form.as_mut() {
            if let Some(field) = f.fields.get_mut(f.cursor) {
                field.value.push(c);
            }
        }
    }

    /// Delete the last character of the focused field.
    pub fn form_backspace(&mut self) {
        if let Some(f) = self.form.as_mut() {
            if let Some(field) = f.fields.get_mut(f.cursor) {
                field.value.pop();
            }
        }
    }

    /// Enter within the form: advance to the next field, or — on the final field
    /// (or an empty form) — return the `Invoke` command to send. The caller sends
    /// it and calls [`App::close_form`].
    pub fn form_enter(&mut self) -> Option<Command> {
        let form = self.form.as_mut()?;
        if form.on_last() {
            Some(Command::Invoke(form.to_invoke()))
        } else {
            form.cursor += 1;
            None
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

    /// The most recently started task that hasn't finished yet, if any --
    /// what `c` (cancel) targets. `tasks` has newest first (see
    /// `upsert_task`'s `insert(0, ..)`), so this is simply the first
    /// unfinished row.
    pub fn running_task(&self) -> Option<TaskId> {
        self.tasks.iter().find(|t| t.status.is_none()).map(|t| t.id)
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
                    actions: vec![ActionSpec {
                        name: "scan".into(),
                        description: None,
                        params: vec![
                            ParamSpec::required("target", "host", "IP or hostname to scan"),
                            ParamSpec::optional("ports", "port-spec", "range/list")
                                .with_default("common ports")
                                .with_example("1-1024"),
                            ParamSpec::optional("timeout_ms", "int", "per-port timeout").with_default("500"),
                        ],
                        params_schema: None,
                    }],
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
            peripherals: vec![],
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
    fn open_form_builds_empty_fields_from_params() {
        let mut app = App::new("x".into());
        app.set_manifest(sample_manifest());
        assert_eq!(app.selected, 1); // net.ports scan
        assert!(app.open_form());
        assert!(app.focus == Focus::Form);
        let form = app.form.as_ref().unwrap();
        assert_eq!(form.module, ModuleId::from("net.ports"));
        assert_eq!(form.action, "scan");
        assert_eq!(form.fields.len(), 3);
        assert_eq!(form.fields[0].name, "target");
        assert!(form.fields[0].required);
        // Fields start empty even though `ports` declares a (prose) default.
        assert!(form.fields.iter().all(|f| f.value.is_empty()));
        assert_eq!(form.cursor, 0);
    }

    #[test]
    fn form_tab_cycles_fields_both_ways() {
        let mut app = App::new("x".into());
        app.set_manifest(sample_manifest());
        app.open_form();
        app.form_next();
        assert_eq!(app.form.as_ref().unwrap().cursor, 1);
        app.form_next();
        app.form_next(); // wraps 2 -> 0
        assert_eq!(app.form.as_ref().unwrap().cursor, 0);
        app.form_prev(); // wraps 0 -> 2
        assert_eq!(app.form.as_ref().unwrap().cursor, 2);
    }

    #[test]
    fn form_char_and_backspace_edit_focused_field() {
        let mut app = App::new("x".into());
        app.set_manifest(sample_manifest());
        app.open_form();
        for c in "127.0.0.1".chars() {
            app.form_char(c);
        }
        app.form_backspace();
        assert_eq!(app.form.as_ref().unwrap().fields[0].value, "127.0.0.");
        // Editing follows the cursor to the next field.
        app.form_next();
        app.form_char('8');
        app.form_char('0');
        assert_eq!(app.form.as_ref().unwrap().fields[1].value, "80");
        assert_eq!(app.form.as_ref().unwrap().fields[0].value, "127.0.0.");
    }

    #[test]
    fn form_enter_advances_then_submits_with_inferred_types() {
        let mut app = App::new("x".into());
        app.set_manifest(sample_manifest());
        app.open_form();
        for c in "127.0.0.1".chars() {
            app.form_char(c);
        }
        assert!(app.form_enter().is_none(), "enter on field 0 only advances");
        assert_eq!(app.form.as_ref().unwrap().cursor, 1);
        for c in "[1,1024]".chars() {
            app.form_char(c);
        }
        assert!(app.form_enter().is_none(), "enter on field 1 only advances");
        for c in "200".chars() {
            app.form_char(c);
        }
        let cmd = app.form_enter().expect("enter on the final field submits");
        match cmd {
            Command::Invoke(inv) => {
                assert_eq!(inv.module, ModuleId::from("net.ports"));
                assert_eq!(inv.action, "scan");
                assert_eq!(inv.params.0["target"], serde_json::json!("127.0.0.1"));
                assert_eq!(inv.params.0["ports"], serde_json::json!([1, 1024]));
                assert_eq!(inv.params.0["timeout_ms"], serde_json::json!(200));
            }
            _ => panic!("expected an invoke command"),
        }
    }

    #[test]
    fn form_omits_empty_fields() {
        let mut app = App::new("x".into());
        app.set_manifest(sample_manifest());
        app.open_form();
        for c in "10.0.0.1".chars() {
            app.form_char(c); // fill only `target`, leave ports/timeout_ms blank
        }
        match app.form_enter() {
            None => {}
            Some(_) => panic!("should advance, not submit, from field 0"),
        }
        // Jump to the last field and submit without typing anything there.
        app.form_next(); // -> ports (empty)
        let cmd = app.form_enter().expect("last field submits");
        match cmd {
            Command::Invoke(inv) => {
                let obj = inv.params.0.as_object().unwrap();
                assert_eq!(obj.len(), 1, "empty fields are omitted");
                assert_eq!(obj["target"], serde_json::json!("10.0.0.1"));
                assert!(!obj.contains_key("ports"));
                assert!(!obj.contains_key("timeout_ms"));
            }
            _ => panic!("expected an invoke command"),
        }
    }

    #[test]
    fn empty_form_submits_immediately_and_close_resets_focus() {
        let mut app = App::new("x".into());
        app.set_manifest(sample_manifest());
        app.move_down(); // -> wifi.deauth (no declared params), index 3
        assert_eq!(app.selected, 3);
        app.open_form();
        assert!(app.form.as_ref().unwrap().fields.is_empty());
        match app.form_enter().expect("an empty form submits on the first Enter") {
            Command::Invoke(inv) => {
                assert_eq!(inv.action, "start");
                assert!(inv.params.0.as_object().unwrap().is_empty());
            }
            _ => panic!("expected an invoke command"),
        }
        app.close_form();
        assert!(app.form.is_none());
        assert!(app.focus == Focus::Modules);
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

    #[test]
    fn running_task_is_the_newest_unfinished_one() {
        let mut app = App::new("x".into());
        assert!(app.running_task().is_none(), "nothing started yet");

        let first = TaskId::new();
        app.apply(Envelope::new(Body::Ack(Ack { task: first }), 0));
        assert_eq!(app.running_task(), Some(first));

        // A second task starts while the first is still running -- the
        // newest unfinished one wins (tasks are newest-first).
        let second = TaskId::new();
        app.apply(Envelope::new(Body::Ack(Ack { task: second }), 0));
        assert_eq!(app.running_task(), Some(second));

        app.apply(Envelope::new(
            Body::Result(TaskResult { task: second, status: TaskStatus::Ok, output: RawParams::default() }),
            0,
        ));
        assert_eq!(app.running_task(), Some(first), "falls back to the still-running one");

        app.apply(Envelope::new(
            Body::Result(TaskResult { task: first, status: TaskStatus::Cancelled, output: RawParams::default() }),
            0,
        ));
        assert!(app.running_task().is_none(), "both tasks finished");
    }
}

